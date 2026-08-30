use std::{collections::VecDeque, path::PathBuf};

use tact::{
    plugin::{PluginEvent, PluginRequest},
    skill::SharedSkillRegistry,
};
use tact_protocol::{AccountUpdate, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{i18n::Language, theme::Theme};

pub(crate) mod app;
mod file_picker;
mod input_history;
mod slash_command;

mod task_dag;
pub(crate) mod task_panel {
    pub(crate) use agent_tui_kit::state::task_panel::*;
}
mod voice;

pub(crate) use agent_tui_kit::state::account::AccountState;
pub(crate) use file_picker::FilePicker;
pub(crate) use input_history::InputHistory;
pub(crate) use slash_command::SlashCommandState;

pub(crate) use agent_tui_kit::state::log::{LogCoordinator, LogItemKind, SystemMsgStyle};
pub(crate) use agent_tui_kit::state::log_scroll::LogScroll;
#[allow(unused_imports)] // PopupHitRow is re-exported for tui's test code (hit-row helpers)
pub(crate) use agent_tui_kit::state::mouse_state::{
    LogSelection, MouseState, PopupHitRow, PopupTextHit, TextPosition,
};
pub(crate) use agent_tui_kit::state::select_popup::SelectPopup;
pub(crate) use agent_tui_kit::state::selection::PopupTextSelection;
pub(crate) use agent_tui_kit::state::thinking::{
    ActiveThinkingBlock, ThinkingBlock, ThinkingPopup,
};
pub(crate) use agent_tui_kit::state::tool_state::{DiffPopup, SubagentPopup};
pub(crate) use agent_tui_kit::state::ui_types::{
    CodeBlock, CodePopup, FocusedPanel, InputMode, MermaidBlock, MermaidPopup, Status,
    SystemPromptPopup,
};
pub use agent_tui_kit::state::ui_types::{HistoryEntry, SkillEntry};
pub(crate) use app::messages::{find_task_stats_copy_button, is_task_stats_line};
pub(crate) use app::pending::PendingMessage;
pub(crate) use task_dag::{DEFAULT_DAG_RENDER_WIDTH, TaskDagPopup, render_task_dag_lines};
pub(crate) use voice::{VoiceEventOutcome, VoicePhase, VoiceStartResult, VoiceState};

// ========== Basic Types ==========

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
    (
        "subagent_cancel",
        "Cancel a running subagent (usage: /subagent_cancel <child-id>)",
    ),
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

/// Why the select popup is open (agent permission vs `/model` flow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectKind {
    /// Agent `RequestSelect` — confirm emits a `UiResponse` on the command channel.
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

/// A queued agent-originated select (`RequestSelect` / `RequestMultiSelect`)
/// waiting behind the currently-open one. Concurrent subagents can each ask
/// for permission; a single [`SelectPopup`] would overwrite the first waiter
/// and hang it, so these queue up and are shown one at a time.
pub(crate) struct AgentSelectRequest {
    pub prompt: String,
    pub options: Vec<String>,
    pub request_id: u64,
    pub multi: bool,
    pub log_confirm: bool,
}

// ========== Main State ==========

/// TUI application main state, holding all UI state, scroll positions,
/// communication channels, and current mode.
pub struct App {
    // Input
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) input_scroll: u16,
    /// Messages queued while the agent is busy (Codex-style "submit after the
    /// current task"); auto-submitted on Idle/Done, or immediately on Esc.
    pub(crate) pending_messages: Vec<PendingMessage>,
    /// Hit area of the pending block's `[Cancel]` button (drops the queue
    /// without touching the running task; `Rect::default()` = inactive).
    pub(crate) pending_cancel_btn_area: ratatui::layout::Rect,
    pub(crate) cmd_line: String,
    /// Model context window in tokens (from agent config `model_context_window`).
    pub(crate) model_context_window: usize,
    pub(crate) log: LogCoordinator,
    pub(crate) status: Status,
    /// Component registry (whole-App switch, plan step 9): owns the plan,
    /// thinking, stream, tool, status-bar and task-panel components; the shell
    /// reads/mutates their state via the typed accessors in `app/registry.rs`
    /// and routes updates through `dispatch_components` (agent.rs). The shared
    /// `LogCoordinator` stays shell-owned (decision in task #42).
    pub(crate) registry: agent_tui_kit::components::ComponentRegistry,
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
    /// Last uptime whole-second that triggered an idle dirty tick (dedupe redraws).
    pub(crate) last_uptime_tick_secs: Option<i64>,
    /// Last git branch refresh time (throttle to avoid running `git` too often).
    pub(crate) last_git_refresh: Option<std::time::Instant>,
    /// Current working directory.
    pub(crate) workspace_dir: String,
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
    /// Agent-originated selects queued behind the currently-open one
    /// (concurrent subagents asking for permission simultaneously).
    pub(crate) pending_agent_selects: VecDeque<AgentSelectRequest>,
    // File picker popup (triggered by @ in insert mode)
    pub(crate) file_picker: FilePicker,
    pub(crate) slash_command: SlashCommandState,
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
