// impl App — core application logic
// Extracted from state.rs to keep file sizes manageable.

use crate::state::{
    App, CodeBlock, DiffBlock, DiffPopup, FocusedPanel, HistoryEntry, InputHistory, InputMode,
    LogScroll, MouseState, PlanPanel, SearchState, SelectPopup, Status, StatusBarState,
    StreamState, ThinkingBlock, ThinkingPopup, ThinkingState,
};
use crate::render::render_md::{format_table, is_horizontal_rule, render_markdown_tui};
use crate::i18n::{Language, Messages};
use crate::theme::{Theme, ThemeName};
use arboard::Clipboard;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Local;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, ScrollbarState};
use std::path::{Path, PathBuf};
use tact_core::{AgentErrorKind, AgentUpdate, StepStatus, UserCommand};

const CODE_BG: Color = Color::Rgb(30, 35, 50);
const CODE_FG: Color = Color::Rgb(200, 200, 210);
const STREAMING_INDICATOR: &str = " ▌";
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

impl App {
    /// Create an initialized App instance, defaulting to Insert mode with the Nord theme.
    pub(crate) fn new(
        agent_rx: UnboundedReceiver<AgentUpdate>,
        user_cmd_tx: UnboundedSender<UserCommand>,
        work_dir: PathBuf,
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
        Self {
            input: String::new(),
            input_cursor: 0,
            input_scroll: 0,
            cmd_line: String::new(),
            messages: Vec::new(),
            visible_indices: Vec::new(),
            raw_messages: Vec::new(),
            plan: PlanPanel::new(),
            status: Status::Idle,
            agent_rx,
            user_cmd_tx,
            task_history: Vec::new(),
            theme: Theme::by_name(ThemeName::Nord),
            log_scroll: LogScroll::new(),
            show_history: false,
            show_help: false,
            focused_panel: FocusedPanel::Log,
            mouse: MouseState::new(),
            input_mode: InputMode::Insert,
            palette_selected: 0,
            search: SearchState::new(),
            command_history: Vec::new(),
            input_history: InputHistory::new(Self::load_history(&work_dir)),
            work_dir,
            should_quit: false,
            dirty: true,
            clipboard_buffer: String::new(),
            status_bar: StatusBarState::new(git_branch),
            task_start_time: None,
            task_done_time: None,
            process_start_time: chrono::Local::now(),
            workspace_dir,
            select: SelectPopup::new(),
            diff_blocks: Vec::new(),
            diff_popup: None,
            code_blocks: Vec::new(),
            code_popup: None,
            stream: StreamState::new(),
            thinking: ThinkingState::new(),
            balance_info: None,
            party_mode: false,
            konami_progress: 0,
            language: Language::English,
            flash_msg: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }


}
