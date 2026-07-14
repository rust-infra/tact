// impl App — core application logic
// Extracted from state.rs to keep file sizes manageable.

use crate::i18n::Language;
use crate::theme::Theme;
use crate::widgets::state::{
    AccountState, App, FilePicker, FocusedPanel, InputHistory, InputMode, LogScroll, MouseState,
    PlanPanel, SelectKind, SelectPopup, SkillEntry, SlashCommandState, Status, StatusBarState,
    StreamState, ThinkingState, ToolState,
};
use std::path::PathBuf;
use tact_protocol::{AccountUpdate, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

impl App {
    /// Create an initialized App instance, defaulting to Insert mode with the Retro theme.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent_rx: UnboundedReceiver<AgentUpdate>,
        account_rx: Option<UnboundedReceiver<AccountUpdate>>,
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
        Self {
            input: String::new(),
            input_cursor: 0,
            input_scroll: 0,
            cmd_line: String::new(),
            context_limit_chars: 500_000,
            messages: Vec::new(),
            raw_messages: Vec::new(),
            raw_message_types: Vec::new(),
            plan: PlanPanel::default(),
            status: Status::Idle,
            agent_rx,
            account_rx,
            user_cmd_tx,
            task_history: Vec::new(),
            theme: Theme::from(theme_name),
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
            status_bar: StatusBarState::new(git_branch),
            task_start_time: None,
            last_prompt_elapsed_secs: None,
            task_done_time: None,
            process_start_time: chrono::Local::now(),
            workspace_dir,
            select: SelectPopup::default(),
            select_kind: SelectKind::Agent,
            file_picker: FilePicker::new(),
            slash_command: SlashCommandState::default(),
            tools: ToolState::default(),
            code_blocks: Vec::new(),
            code_popup: None,
            stream: StreamState::default(),
            thinking: ThinkingState::default(),
            account: AccountState::default(),
            skills_description,
            skills_data,
            spinner_frame: 0,
            loading_idx: None,
            panel_split_ratio: 0.20,
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
