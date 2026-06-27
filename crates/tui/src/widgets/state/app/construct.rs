// impl App — core application logic
// Extracted from state.rs to keep file sizes manageable.

use crate::i18n::Language;
use crate::theme::Theme;
use crate::widgets::state::{
    App, FilePicker, FocusedPanel, InputHistory,
    InputMode, LogScroll, MouseState, PlanPanel, SearchState, SelectPopup, SlashCommandState, Status, StatusBarState,
    StreamState, ThinkingState, ToolState,
};
use std::path::{Path, PathBuf};
use tact_protocol::{AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

impl App {
    /// Create an initialized App instance, defaulting to Insert mode with the Retro theme.
    pub(crate) fn new(
        agent_rx: UnboundedReceiver<AgentUpdate>,
        user_cmd_tx: UnboundedSender<UserCommand>,
        work_dir: PathBuf,
        input_history_entries: Vec<String>,
        session_id: String,
        history_save_tx: UnboundedSender<(String, String)>,
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
        let detected_theme_name = crate::theme_detection::detect_theme();
        Self {
            input: String::new(),
            input_cursor: 0,
            input_scroll: 0,
            cmd_line: String::new(),
            messages: Vec::new(),
            raw_messages: Vec::new(),
            raw_message_types: Vec::new(),
            plan: PlanPanel::new(),
            status: Status::Idle,
            agent_rx,
            user_cmd_tx,
            task_history: Vec::new(),
            theme: Theme::by_name(detected_theme_name),
            log_scroll: LogScroll::new(),
            show_history: false,
            show_help: false,
            focused_panel: FocusedPanel::Log,
            mouse: MouseState::new(),
            input_mode: InputMode::Insert,
            palette_selected: 0,
            search: SearchState::new(),
            command_history: Vec::new(),
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
            select: SelectPopup::new(),
            file_picker: FilePicker::new(),
            slash_command: SlashCommandState::default(),
            tools: ToolState::new(),
            code_blocks: Vec::new(),
            code_popup: None,
            stream: StreamState::new(),
            thinking: ThinkingState::new(),
            balance_info: None,
            party_mode: false,
            konami_progress: 0,
            spinner_frame: 0,
            loading_idx: None,
            panel_split_ratio: 0.20,
            language: Language::English,
            flash_msg: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Open the `@` file picker: scan the project root for files (skipping
    /// hidden directories and common build/output folders), populate the picker
    /// options, and switch to `FilePicker` mode.
    pub(crate) fn open_file_picker(&mut self) {
        let mut options = Vec::new();
        collect_files(&self.work_dir, &self.work_dir, &mut options, 200);
        options.sort();
        self.file_picker.set(options);
        self.input_mode = InputMode::FilePicker;
    }
}

const FILE_PICKER_EXCLUDES: &[&str] = &[".git", "target", "node_modules", ".tact"];

fn collect_files(dir: &Path, base: &Path, options: &mut Vec<String>, max: usize) {
    if options.len() >= max {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(it) => it.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || FILE_PICKER_EXCLUDES.contains(&name) {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, base, options, max);
            if options.len() >= max {
                break;
            }
        } else if let Ok(rel) = path.strip_prefix(base) {
            options.push(rel.to_string_lossy().to_string());
            if options.len() >= max {
                break;
            }
        }
    }
}
