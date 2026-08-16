use std::path::PathBuf;

use ratatui::text::Line;
use tact::{
    plugin::{PluginEvent, PluginRequest},
    skill::SharedSkillRegistry,
};
pub(crate) use tact_protocol::PlanStep;
use tact_protocol::{AccountUpdate, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{i18n::Language, theme::Theme};

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

mod task_dag;
pub(crate) mod task_panel;
mod thinking_state;
mod tool_state;
mod voice;

pub(crate) use account::AccountState;
pub(crate) use file_picker::FilePicker;
pub(crate) use input_history::InputHistory;
pub(crate) use log_scroll::LogScroll;
pub(crate) use mouse_state::{LogSelection, MouseState, PopupHitRow, PopupTextHit, TextPosition};
pub(crate) use plan_panel::PlanPanel;
pub(crate) use select_popup::SelectPopup;
pub(crate) use slash_command::SlashCommandState;
pub(crate) use status_bar_state::StatusBarState;
pub(crate) use stream_state::StreamState;

pub(crate) use app::messages::{TASK_STATS_COPY_BTN, is_task_stats_line};
pub(crate) use task_dag::{DEFAULT_DAG_RENDER_WIDTH, TaskDagPopup, render_task_dag_lines};
pub(crate) use task_panel::TaskPanelState;
pub(crate) use thinking_state::{ActiveThinkingBlock, ThinkingBlock, ThinkingPopup, ThinkingState};
pub(crate) use tool_state::{
    ActiveToolBlock, DiffPopup, PopupTextSelection, SubagentPopup, ToolBlock, ToolState,
};
pub(crate) use voice::{VoiceEventOutcome, VoicePhase, VoiceStartResult, VoiceState};

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
    ("model-subagent", "Switch subagent model"),
    ("permission", "Set permission mode (Default/Plan/Auto)"),
    ("view-system-prompt", "View system prompt"),
    ("save", "Save log to file"),
    ("compact", "Compact conversation history"),
    ("cancel", "Cancel current task"),
    ("quit", "Quit application"),
    ("help", "Show help panel"),
    ("history", "Show task history"),
    ("skills", "List available skills"),
    ("skill-reload", "Reload skills from disk"),
    ("plugin", "Manage plugins and marketplaces"),
    ("balance", "Query account balance (DeepSeek/Kimi)"),
    ("lang", "Toggle language (EN/中文)"),
    ("stats", "Show session statistics"),
    ("tasks-dag", "Show task dependency DAG"),
    ("background", "Check background task status"),
];

/// A skill available in the TUI slash / palette picker.
///
/// Enter on a skill **invokes** immediately from the `/` popup (body wrapped
/// in `<skill>`, with optional `$ARGUMENTS` handling). **Tab** only fills
/// `/name ` so args can be edited first. Built-in command names take priority
/// and exclude colliding skills from [`App::palette_commands`].
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    /// Markdown body after frontmatter (from disk at load / skill-reload time).
    pub body: String,
}

/// Why the select popup is open (agent permission vs `/model` flow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectKind {
    /// Agent `RequestSelect` — confirm sends oneshot reply.
    Agent,
    /// `/model` first step — choose a model before applying either value.
    ModelPick,
    /// `/model` second step (effort-semantic models: openai/deepseek/kimi k3)
    /// — choose a reasoning effort before applying. `efforts` = selectable
    /// tiers for this model (mapped or provider default).
    ModelProfileEffortPick {
        model: String,
        efforts: Vec<tact_llm::OpenAiReasoningEffort>,
    },
    /// `/model` second step — choose a thinking budget before applying.
    /// `budgets` = selectable tiers for this model (mapped or default 5).
    ThinkBudgetPick {
        model: String,
        budgets: Vec<usize>,
    },
    /// Optional combined "save to config?" prompt after session application.
    PersistModelAndBudget {
        model: String,
        thinking_budget: usize,
    },
    /// Optional "save model + reasoning effort to config?" prompt.
    PersistModelAndEffort {
        model: String,
        effort: tact_llm::OpenAiReasoningEffort,
    },
    /// Prompt source selection for `/view-system-prompt`.
    ViewSystemPrompt,
    /// `/permission` picker — choose Default / Plan / Auto.
    PermissionModePick,
    /// `/model-subagent` flow
    SubagentModelPick,
    SubagentModelProfileEffortPick {
        model: String,
        efforts: Vec<tact_llm::OpenAiReasoningEffort>,
    },
    SubagentThinkBudgetPick {
        model: String,
        budgets: Vec<usize>,
    },
    SubagentPersistModelAndBudget {
        model: String,
        thinking_budget: usize,
    },
    SubagentPersistModelAndEffort {
        model: String,
        effort: tact_llm::OpenAiReasoningEffort,
    },
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FocusedPanel {
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

/// A successfully rendered Mermaid diagram spliced into the log as terminal art.
///
/// Unlike [`CodeBlock`], there is no card chrome — `start_idx..end_idx` covers
/// the diagram rows themselves. Double-click opens [`MermaidPopup`] so the
/// original fence body can be copied.
#[derive(Debug, Clone)]
pub(crate) struct MermaidBlock {
    /// First diagram line index in messages (inclusive).
    pub start_idx: usize,
    /// One-past-last diagram line index in messages.
    pub end_idx: usize,
    /// Fence body only (no ```mermaid / closing ```).
    pub source: String,
}

/// Code block popup state (similar to ThinkingPopup / DiffPopup).
#[derive(Debug, Clone)]
pub(crate) struct CodePopup {
    pub block_idx: usize,
    pub lang: String,
    pub scroll: u16,
}

/// Mermaid source popup (double-click a rendered diagram in the log).
#[derive(Debug, Clone)]
pub(crate) struct MermaidPopup {
    pub block_idx: usize,
    pub scroll: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct SystemPromptPopup {
    pub title: String,
    /// Raw Markdown source; laid out width-aware at popup render time.
    pub source: String,
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
    /// Model context window in tokens (from agent config `model_context_window`).
    pub(crate) model_context_window: usize,
    pub(crate) messages: Vec<Line<'static>>,
    pub(crate) raw_messages: Vec<String>,
    pub(crate) raw_message_types: Vec<RawMessageType>,
    /// Parallel to `raw_messages`: cached `MarkdownCell` when the message is
    /// a whole-markdown notice (`AgentUpdate::MdInfo` / `/skills`), `None`
    /// otherwise. `Some` doubles as the "render as MarkdownCell" marker.
    pub(crate) markdown_cells: Vec<Option<crate::render::cells::markdown::MarkdownCell>>,
    pub(crate) plan: PlanPanel,
    pub(crate) status: Status,
    pub(crate) agent_rx: UnboundedReceiver<AgentUpdate>,
    pub(crate) account_rx: Option<UnboundedReceiver<AccountUpdate>>,
    pub(crate) plugin_rx: UnboundedReceiver<PluginEvent>,
    pub(crate) plugin_tx: UnboundedSender<PluginRequest>,
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
    /// Persistent task progress sticky (under Log).
    pub(crate) task_panel: TaskPanelState,
    /// Task completion time (for top status bar Done highlight timer;
    /// auto-reverts to Idle display after 2s).
    pub(crate) task_done_time: Option<chrono::DateTime<chrono::Local>>,
    /// Process start time (for bottom status bar showing total TUI uptime).
    pub(crate) process_start_time: chrono::DateTime<chrono::Local>,
    /// Last uptime whole-second that triggered an idle dirty tick (dedupe redraws).
    pub(crate) last_uptime_tick_secs: Option<i64>,
    /// Last git branch refresh time (throttle to avoid running `git` too often).
    pub(crate) last_git_refresh: Option<std::time::Instant>,
    /// Current working directory.
    pub(crate) workspace_dir: String,
    /// Tool invocation blocks and diff popup state.
    pub(crate) tools: ToolState,
    /// Completed LLM code block overlays.
    pub(crate) code_blocks: Vec<CodeBlock>,
    /// Code block popup preview (fullscreen independent scroll viewer).
    pub(crate) code_popup: Option<CodePopup>,
    /// Successfully rendered Mermaid diagrams (source retained for copy popup).
    pub(crate) mermaid_blocks: Vec<MermaidBlock>,
    /// Mermaid source popup (double-click a rendered diagram).
    pub(crate) mermaid_popup: Option<MermaidPopup>,
    /// `/tasks-dag` Mermaid→Unicode dependency graph popup.
    pub(crate) task_dag_popup: Option<TaskDagPopup>,
    /// Subagent live-output / markdown summary popup.
    pub(crate) subagent_popup: Option<SubagentPopup>,
    pub(crate) system_prompt_popup: Option<SystemPromptPopup>,
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
    /// Voice-to-text title-bar button and worker channels.
    pub(crate) voice: VoiceState,
    /// Keyboard shortcut to start/stop voice recording, parsed from config.
    /// `None` means mouse-only (no keyboard shortcut).
    pub(crate) voice_parsed_keybind:
        Option<(crossterm::event::KeyModifiers, crossterm::event::KeyCode)>,
    /// Cached account balance / usage quota state from the account service.
    pub(crate) account: AccountState,
    /// List of available skills (name + description lines).
    pub(crate) skills_description: String,
    /// Skills for `/name` slash + palette picker.
    pub(crate) skills_data: Vec<SkillEntry>,
    /// Same mutex as agent `ToolContext.skill_registry` (interactive mode).
    pub(crate) skill_registry: SharedSkillRegistry,
    /// Shared session store used to inspect persisted request payloads.
    pub(crate) session_store: Option<tact::store::DynSessionStore>,
    /// Spinner animation frame (0-9) for typing/loading indicator.
    pub(crate) spinner_frame: u8,
    /// Loading placeholder index in messages (spinner row while waiting for output).
    pub(crate) loading_idx: Option<usize>,
    /// Current interface language.
    pub(crate) language: Language,
    /// Brief status bar notification (auto-clears after 3s).
    pub(crate) flash_msg: Option<(String, std::time::Instant)>,
    /// Input box undo stack (max 100, snapshot saved before each change).
    pub(crate) undo_stack: Vec<(String, usize)>,
    /// Input box redo stack.
    pub(crate) redo_stack: Vec<(String, usize)>,
}
