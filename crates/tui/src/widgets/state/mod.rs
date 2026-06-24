use crate::i18n::Language;
use crate::theme::Theme;
use crate::widgets::tool_widget::ToolRenderOutput;
use chrono;
use ratatui::text::Line;
use std::path::PathBuf;
use tact_core::{AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub(crate) use tact_core::PlanStep;

pub(crate) mod app;
mod file_picker;
mod input_history;
mod log_scroll;
mod mouse_state;
mod plan_panel;
mod search_state;
mod select_popup;
mod status_bar_state;
mod stream_state;
mod thinking_state;

pub(crate) use file_picker::FilePicker;
pub(crate) use input_history::InputHistory;
pub(crate) use log_scroll::LogScroll;
pub(crate) use mouse_state::MouseState;
pub(crate) use plan_panel::PlanPanel;
pub(crate) use search_state::SearchState;
pub(crate) use select_popup::SelectPopup;
pub(crate) use status_bar_state::StatusBarState;
pub(crate) use stream_state::StreamState;
pub(crate) use thinking_state::{ThinkingBlock, ThinkingPopup, ThinkingState};

// ========== Basic Types ==========

/// Current keyboard input mode, determining how key presses are interpreted.
#[derive(PartialEq)]
pub(crate) enum InputMode {
    Normal,
    Insert,
    Search,
    Palette,
    Select,
    FilePicker,
}

/// Commands shown in the command palette (triggered by `:`).
pub(crate) const PALETTE_COMMANDS: &[(&str, &str)] = &[
    ("theme", "Toggle color theme"),
    ("save", "Save log to file"),
    ("cancel", "Cancel current task"),
    ("quit", "Quit application"),
    ("help", "Show help panel"),
    ("history", "Show task history"),
    ("search", "Search log messages"),
    ("balance", "Query account balance (DeepSeek)"),
    ("lang", "Toggle language (EN/中文)"),
];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FocusedPanel {
    Plan,
    Log,
}

#[derive(Clone)]
pub struct HistoryEntry {
    pub task: String,
    pub timestamp: String,
    pub summary: String,
}

// ========== Tool Types ==========

/// State for a tool invocation result that includes a detail card.
#[derive(Debug, Clone)]
pub(crate) struct ToolBlock {
    /// Physical index of the first placeholder row in `messages` / `raw_messages`.
    pub phys_idx: usize,
    /// Pre-built render output from [`ToolWidget`].
    pub output: ToolRenderOutput,
}

/// Popup preview state for file write content.
#[derive(Debug, Clone)]
pub(crate) struct DiffPopup {
    pub file_path: String,
    pub scroll: u16,
    /// Lazily-loaded full file content. `None` until first render/population.
    pub cached_content: Option<String>,
    /// Pre-rendered syntax-highlighted lines (built once when cached_content is loaded).
    /// Empty Vec means "not yet highlighted" (or empty file).
    pub highlighted_lines: Vec<Line<'static>>,
}

/// A completed LLM code block, rendered as a card overlay in the log panel.
#[derive(Debug, Clone)]
pub(crate) struct CodeBlock {
    /// First placeholder line index in messages (inclusive).
    pub start_idx: usize,
    /// One-past-last placeholder line index in messages.
    pub end_idx: usize,
    pub lang: String,
    /// Raw source lines (without ``` fences), used for copy and rendering.
    pub content: String,
    /// Pre-rendered styled lines for the card interior.
    pub styled: Vec<Line<'static>>,
}

/// Code block popup state (similar to ThinkingPopup / DiffPopup).
#[derive(Debug, Clone)]
pub(crate) struct CodePopup {
    pub block_idx: usize,
    pub lang: String,
    pub scroll: u16,
}

// ========== Execution State ==========

/// Current agent execution state, driving the status bar and UI feedback.
pub(crate) enum Status {
    Idle,
    Planning,
    Executing {
        current_step: usize,
        total: usize,
    },
    WaitingForUser {
        prompt: String,
        step_index: usize,
        approval_tx: tokio::sync::oneshot::Sender<bool>,
    },
    Done,
}

// ========== Main State ==========

/// TUI application main state, holding all UI state, scroll positions,
/// communication channels, and current mode.
pub struct App {
    // Input
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) input_scroll: u16,
    pub(crate) cmd_line: String,
    pub(crate) messages: Vec<Line<'static>>,
    pub(crate) raw_messages: Vec<String>,
    pub(crate) plan: PlanPanel,
    pub(crate) status: Status,
    pub(crate) agent_rx: UnboundedReceiver<AgentUpdate>,
    pub(crate) user_cmd_tx: UnboundedSender<UserCommand>,
    pub(crate) task_history: Vec<HistoryEntry>,
    pub(crate) theme: Theme,
    // Scroll
    pub(crate) log_scroll: LogScroll,
    // Panels
    pub(crate) show_history: bool,
    pub(crate) show_help: bool,
    pub(crate) focused_panel: FocusedPanel,
    // Mouse interaction
    pub(crate) mouse: MouseState,
    // Mode
    pub(crate) input_mode: InputMode,
    // Command palette
    pub(crate) palette_selected: usize,
    // Search
    pub(crate) search: SearchState,
    // Command history (brief)
    pub(crate) command_history: Vec<String>,
    /// User input history.
    pub(crate) input_history: InputHistory,
    /// Project root directory.
    pub(crate) work_dir: PathBuf,
    /// Current session id for scoping persisted input history.
    pub(crate) session_id: String,
    /// Channel for persisting input history to sqlite.
    pub(crate) history_save_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    pub(crate) should_quit: bool,
    /// Dirty flag: set to true on input events, agent updates, or size changes;
    /// skips pointless repaints while idle.
    pub(crate) dirty: bool,
    /// Internal clipboard buffer (used when system clipboard is unavailable).
    pub(crate) clipboard_buffer: String,
    // Bottom status bar
    pub(crate) status_bar: StatusBarState,
    /// Current task start time (for bottom status bar timer).
    pub(crate) task_start_time: Option<chrono::DateTime<chrono::Local>>,
    /// Task completion time (for top status bar Done highlight timer;
    /// auto-reverts to Idle display after 2s).
    pub(crate) task_done_time: Option<chrono::DateTime<chrono::Local>>,
    /// Process start time (for bottom status bar showing total TUI uptime).
    pub(crate) process_start_time: chrono::DateTime<chrono::Local>,
    /// Current working directory.
    pub(crate) workspace_dir: String,
    /// File write diff block list.
    pub(crate) tool_blocks: Vec<ToolBlock>,
    /// File write content popup preview.
    pub(crate) diff_popup: Option<DiffPopup>,
    /// Completed LLM code block overlays.
    pub(crate) code_blocks: Vec<CodeBlock>,
    /// Code block popup preview (fullscreen independent scroll viewer).
    pub(crate) code_popup: Option<CodePopup>,
    // Selection popup
    pub(crate) select: SelectPopup,
    // File picker popup (triggered by @ in insert mode)
    pub(crate) file_picker: FilePicker,
    // Streaming output state
    pub(crate) stream: StreamState,
    // Thinking state
    pub(crate) thinking: ThinkingState,
    /// DeepSeek account balance info (queried once on load and cached).
    pub(crate) balance_info: Option<tact_core::BalanceInfo>,
    /// Party mode: easter egg triggered by Konami Code.
    pub(crate) party_mode: bool,
    /// Konami Code input progress (0 = not started, 1–10 = in progress, 10 = triggered).
    pub(crate) konami_progress: u8,
    /// Spinner animation frame (0-9) for typing/loading indicator.
    pub(crate) spinner_frame: u8,
    /// Loading placeholder index in messages (inserted at PlanGenerated, removed on first content).
    pub(crate) loading_idx: Option<usize>,
    /// Panel split ratio (0.0–1.0) for the Plan panel width. 0.20 = 20% plan, 80% log.
    pub(crate) panel_split_ratio: f64,
    /// Current interface language.
    pub(crate) language: Language,
    /// Brief status bar notification (auto-clears after 3s).
    pub(crate) flash_msg: Option<(String, std::time::Instant)>,
    /// Input box undo stack (max 100, snapshot saved before each change).
    pub(crate) undo_stack: Vec<(String, usize)>,
    /// Input box redo stack.
    pub(crate) redo_stack: Vec<(String, usize)>,
}
