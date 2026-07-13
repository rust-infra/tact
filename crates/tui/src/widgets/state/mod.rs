use crate::i18n::Language;
use crate::theme::Theme;
use ratatui::text::Line;
use std::path::PathBuf;
use tact_protocol::{AccountUpdate, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub(crate) use tact_protocol::PlanStep;

pub(crate) mod account;
pub(crate) mod app;
mod file_picker;
mod input_history;
pub(crate) mod log_messages;
mod log_scroll;
mod mouse_state;
mod plan_panel;
mod select_popup;
mod slash_command;
mod status_bar_state;
mod stream_state;
mod thinking_state;
mod tool_state;

pub(crate) use account::AccountState;
pub(crate) use file_picker::FilePicker;
pub(crate) use input_history::InputHistory;
pub(crate) use log_scroll::LogScroll;
pub(crate) use mouse_state::{LogSelection, MouseState, TextPosition};
pub(crate) use plan_panel::PlanPanel;
pub(crate) use select_popup::SelectPopup;
pub(crate) use slash_command::SlashCommandState;
pub(crate) use status_bar_state::StatusBarState;
pub(crate) use stream_state::StreamState;
pub(crate) use thinking_state::{ThinkingBlock, ThinkingPopup, ThinkingState};
pub(crate) use tool_state::{ActiveToolBlock, DiffPopup, ToolBlock, ToolState};

// ========== Basic Types ==========

/// Current keyboard input mode, determining how key presses are interpreted.
#[derive(PartialEq)]
pub(crate) enum InputMode {
    Normal,
    Insert,
    Palette,
    Select,
    FilePicker,
}

/// Commands shown in the command palette (triggered by `/`).
pub(crate) const PALETTE_COMMANDS: &[(&str, &str)] = &[
    ("theme", "Toggle color theme"),
    ("model", "Switch model for current provider"),
    ("save", "Save log to file"),
    ("cancel", "Cancel current task"),
    ("quit", "Quit application"),
    ("help", "Show help panel"),
    ("history", "Show task history"),
    ("balance", "Query account balance (DeepSeek/Kimi)"),
    ("lang", "Toggle language (EN/中文)"),
];

/// Why the select popup is open (agent permission vs `/model` flow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectKind {
    /// Agent `RequestSelect` — confirm sends oneshot reply.
    Agent,
    /// `/model` picker — confirm applies `set_model` then may open persist prompt.
    ModelPick,
    /// Optional "Save to config?" after a model switch.
    PersistModel { model: String },
}

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

// ========== Code Block Types ==========

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
    Executing { current_step: usize, total: usize },
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum RawMessageType {
    LLM,
    LLMThinking,
    SysTool,
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
    /// Maximum allowed input length (from agent config `context_limit_chars`).
    pub(crate) context_limit_chars: usize,
    pub(crate) messages: Vec<Line<'static>>,
    pub(crate) raw_messages: Vec<String>,
    pub(crate) raw_message_types: Vec<RawMessageType>,
    pub(crate) plan: PlanPanel,
    pub(crate) status: Status,
    pub(crate) agent_rx: UnboundedReceiver<AgentUpdate>,
    pub(crate) account_rx: Option<UnboundedReceiver<AccountUpdate>>,
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
    /// Frozen elapsed seconds from the most recent submitted prompt.
    /// Kept until a new prompt is submitted.
    pub(crate) last_prompt_elapsed_secs: Option<i64>,
    /// Task completion time (for top status bar Done highlight timer;
    /// auto-reverts to Idle display after 2s).
    pub(crate) task_done_time: Option<chrono::DateTime<chrono::Local>>,
    /// Process start time (for bottom status bar showing total TUI uptime).
    pub(crate) process_start_time: chrono::DateTime<chrono::Local>,
    /// Current working directory.
    pub(crate) workspace_dir: String,
    /// Tool invocation blocks and diff popup state.
    pub(crate) tools: ToolState,
    /// Completed LLM code block overlays.
    pub(crate) code_blocks: Vec<CodeBlock>,
    /// Code block popup preview (fullscreen independent scroll viewer).
    pub(crate) code_popup: Option<CodePopup>,
    // Selection popup
    pub(crate) select: SelectPopup,
    /// Distinguishes agent permission selects from `/model` UX.
    pub(crate) select_kind: SelectKind,
    // File picker popup (triggered by @ in insert mode)
    pub(crate) file_picker: FilePicker,
    pub(crate) slash_command: SlashCommandState,
    // Streaming output state
    pub(crate) stream: StreamState,
    // Thinking state
    pub(crate) thinking: ThinkingState,
    /// Cached account balance / usage quota state from the account service.
    pub(crate) account: AccountState,
    /// Spinner animation frame (0-9) for typing/loading indicator.
    pub(crate) spinner_frame: u8,
    /// Loading placeholder index in messages (spinner row while waiting for output).
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
