// impl App — core application logic
// Extracted from state.rs to keep file sizes manageable.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
};

use tact::plugin::{PluginEvent, PluginRequest};
use tact_protocol::{AccountUpdate, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    i18n::Language,
    theme::Theme,
    widgets::state::{
        AccountState, App, FilePicker, FocusedPanel, InputHistory, InputMode, LogCoordinator,
        LogScroll, MouseState, SelectKind, SelectPopup, SkillEntry, SlashCommandState, Status,
        VoiceState,
    },
};
use agent_tui_kit::components::{
    ComponentRegistry, PlanComponent, StatusBarComponent, StreamComponent, SubagentPanelComponent,
    TaskPanelComponent, ThinkingComponent, ToolComponent,
};
use agent_tui_kit::i18n::Messages;

impl App {
    /// Create an initialized App instance, defaulting to Insert mode with the Retro theme.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent_rx: UnboundedReceiver<AgentUpdate>,
        account_rx: Option<UnboundedReceiver<AccountUpdate>>,
        plugin_rx: UnboundedReceiver<PluginEvent>,
        plugin_tx: UnboundedSender<PluginRequest>,
        user_cmd_tx: UnboundedSender<UserCommand>,
        work_dir: PathBuf,
        input_history_entries: Vec<String>,
        session_id: String,
        history_save_tx: UnboundedSender<(String, String)>,
        theme: String,
        skills_description: String,
        skills_data: Vec<SkillEntry>,
    ) -> Self {
        let git_branch = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let workspace_dir = {
            let cwd = std::env::current_dir().ok();
            let home = std::env::var("HOME").ok();
            match (cwd, home) {
                (Some(p), Some(h)) => {
                    let path = p.to_string_lossy().to_string();
                    if path.starts_with(&h) {
                        format!("~{}", &path[h.len()..])
                    } else {
                        path
                    }
                }
                (Some(p), None) => p.to_string_lossy().to_string(),
                _ => "?".to_string(),
            }
        };
        let theme_name = crate::theme_detection::resolve_theme(&theme);
        let theme = Theme::from(theme_name);
        let language = Language::English;
        // Component registry (whole-App switch): components own their state;
        // the shell reads it via the typed accessors and routes updates
        // through `dispatch_components`.
        let mut registry = ComponentRegistry::new();
        registry.push(PlanComponent::new());
        registry.push(ThinkingComponent::new(
            theme,
            Messages::by_language(language),
        ));
        registry.push(StreamComponent::new(theme, Messages::by_language(language)));
        registry.push(ToolComponent::new(theme, Messages::by_language(language)));
        registry.push(StatusBarComponent::new(git_branch));
        registry.push(TaskPanelComponent::new());
        registry.push(SubagentPanelComponent::new());
        Self {
            input: String::new(),
            input_cursor: 0,
            input_scroll: 0,
            pending_messages: Vec::new(),
            pending_cancel_btn_area: ratatui::layout::Rect::default(),
            cmd_line: String::new(),
            model_context_window: 200_000,
            log: LogCoordinator::default(),
            status: Status::Idle,
            agent_rx,
            account_rx,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            task_history: Vec::new(),
            theme,
            log_scroll: LogScroll::new(),
            show_history: false,
            show_help: false,
            focused_panel: FocusedPanel::Log,
            mouse: MouseState::new(),
            input_mode: InputMode::Insert,
            palette_selected: 0,
            input_history: InputHistory::new(input_history_entries),
            work_dir,
            session_id,
            history_save_tx,
            should_quit: false,
            dirty: true,
            clipboard_buffer: String::new(),
            system_clipboard: None,
            task_start_time: None,
            last_prompt_elapsed_secs: None,
            task_done_time: None,
            process_start_time: chrono::Local::now(),
            last_uptime_tick_secs: None,
            last_git_refresh: None,
            git_branch_task: None,
            skills_task: None,
            workspace_dir,
            select: SelectPopup::default(),
            select_kind: SelectKind::Agent,
            pending_agent_selects: VecDeque::new(),
            pending_ui: None,
            file_picker: FilePicker::new(),
            slash_command: SlashCommandState::default(),
            registry,
            code_blocks: Vec::new(),
            code_popup: None,
            mermaid_blocks: Vec::new(),
            mermaid_popup: None,
            task_dag_popup: None,
            subagent_popups: HashMap::new(),
            active_subagent_popup: None,
            system_prompt_popup: None,
            voice: VoiceState::disabled(),
            voice_parsed_keybind: None,
            account: AccountState::default(),
            skills_description,
            skills_data,
            skill_registry: std::sync::Arc::new(std::sync::Mutex::new(
                tact::skill::SkillRegistry::new(std::iter::empty::<std::path::PathBuf>()),
            )),
            session_store: None,
            spinner_frame: 0,
            loading_idx: None,
            language: Language::English,
            flash_msg: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Open the `@` file picker starting at the project root. The picker lists
    /// entries in the current directory only; directories can be entered to
    /// browse their contents.
    pub(crate) fn open_file_picker(&mut self) {
        self.file_picker
            .set_dir(self.work_dir.clone(), self.work_dir.clone());
        self.input_mode = InputMode::FilePicker;
    }
}
