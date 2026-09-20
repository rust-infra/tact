//! The Tact window shell.
//!
//! Five regions, laid out as one continuous workspace: a fixed title bar, a
//! session sidebar, the transcript, a collapsible work pane, and a fixed status
//! bar. The transcript owns a virtualized message scroller and a bottom-docked
//! composer, and is driven by the live agent session when one is attached.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{
    ActiveTheme as _, Root, Sizable as _, Theme, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    command::{Command, CommandState},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    message_scroller::{MessageScroller, MessageScrollerState},
    popover::Popover,
    progress::ProgressCircle,
    scroll::ScrollableElement as _,
    setting::{SettingGroup, SettingItem, SettingPage, Settings},
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Rems, Render, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Subscription, Window, div, px, rems,
};

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use tact_protocol::UiResponse;

use crate::RecentSession;
use crate::commands;
use crate::composer::{self, Attachment};
use crate::pane::{self, FilesPane, WorkPane};
use crate::session::{self, Change, Conversation, Request, SessionHandle, SessionState};
use crate::theme;
use crate::transcript;

/// Fixed title bar height (44 px at the default 16 px rem).
const TITLE_BAR_HEIGHT: Rems = rems(2.75);
/// Sidebar width (260 px at the default rem).
const SIDEBAR_WIDTH: Rems = rems(16.25);
/// Minimum sidebar session count before the list is grouped into date buckets.
const SIDEBAR_GROUP_MIN: usize = 4;
/// Width below which the sidebar becomes an overlay (960 px at the default rem).
const SIDEBAR_OVERLAY_UNDER: Rems = rems(60.);
/// Work pane width (420 px at the default rem).
const WORK_PANE_WIDTH: Rems = rems(26.25);
/// Fixed status bar height (26 px at the default rem).
const STATUS_BAR_HEIGHT: Rems = rems(1.625);
/// Cap on the transcript's text measure (720 px at the default rem), which is
/// the prototype's `.thread { width: min(720px, 100% - 48px) }`.
const TRANSCRIPT_MEASURE: Rems = rems(45.);
/// Width from which the work pane sits in the layout (1280 px at the default
/// rem). Below it the drawer form applies (Phase 6).
const WORK_PANE_IN_FLOW_FROM: Rems = rems(80.);

/// The three top-level workspace presets over one session model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    /// Conversation and quick questions.
    Chat,
    /// Plans, tasks, background runs, and subagents.
    Agent,
    /// Diff and files, with repository state prominent.
    Code,
}

impl Workspace {
    /// Every preset in tab order.
    pub const ALL: [Self; 3] = [Self::Chat, Self::Agent, Self::Code];

    /// Label shown in the title bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Agent => "Agent",
            Self::Code => "Code",
        }
    }

    /// Position of this preset in [`Self::ALL`].
    pub fn index(self) -> usize {
        match self {
            Self::Chat => 0,
            Self::Agent => 1,
            Self::Code => 2,
        }
    }

    /// The preset at `index`, falling back to [`Workspace::Chat`].
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or(Self::Chat)
    }
}

/// One selectable command in the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteCommand {
    OpenPalette,
    ToggleSidebar,
    ToggleWorkPane,
    OpenDiff,
    OpenTasks,
    NewSession,
    FocusComposer,
    StopTask,
    CycleTranscriptDetail,
    CycleSessions,
    CycleSessionsBackward,
    CompactSession,
    SessionStats,
    McpServers,
    OpenSettings,
    ToggleTheme,
}

impl PaletteCommand {
    /// Resolve the model coordinates used by [`Command::on_confirm`].
    fn from_index(index: gpui_kit::component::IndexPath) -> Option<Self> {
        match (index.section, index.row) {
            (0, 0) => Some(Self::OpenPalette),
            (0, 1) => Some(Self::ToggleSidebar),
            (0, 2) => Some(Self::ToggleWorkPane),
            (0, 3) => Some(Self::OpenDiff),
            (0, 4) => Some(Self::OpenTasks),
            (1, 0) => Some(Self::NewSession),
            (1, 1) => Some(Self::FocusComposer),
            (1, 2) => Some(Self::StopTask),
            (1, 3) => Some(Self::CycleTranscriptDetail),
            (1, 4) => Some(Self::CycleSessions),
            (1, 5) => Some(Self::CycleSessionsBackward),
            (1, 6) => Some(Self::CompactSession),
            (1, 7) => Some(Self::SessionStats),
            (1, 8) => Some(Self::McpServers),
            (2, 0) => Some(Self::OpenSettings),
            (2, 1) => Some(Self::ToggleTheme),
            _ => None,
        }
    }
}

/// The application shell view.
pub struct TactApp {
    workspace: Workspace,
    sidebar_open: bool,
    work_pane_open: bool,
    work_pane: WorkPane,
    files: FilesPane,
    /// Lazily loaded `git diff` bodies for the Diff pane.
    diffs: pane::DiffPane,
    /// Transcript rows and the live-row bookkeeping behind them.
    conversation: Conversation,
    /// Session state the panes, composer, and status bar read.
    state: SessionState,
    /// Prompts typed while a turn was in flight, oldest first.
    queued: VecDeque<String>,
    /// Files the user attached to the next composer submission.
    attachments: Vec<Attachment>,
    transcript_state: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    /// Sidebar session filter, bound to the prototype's search field.
    session_search: Entity<InputState>,
    /// The agent session, once one is attached.
    session: Option<SessionHandle>,
    /// Recent sessions for the workspace, newest first.
    recent: Vec<RecentSession>,
    /// Keeps the event pump alive for as long as the window is open.
    _pump: Option<session::Pump>,
    /// Command palette interaction state, retained while its dialog is open.
    palette: Option<Entity<CommandState>>,
    /// Stable focus for the application's window-level shortcut context.
    root_focus: FocusHandle,
    /// How much supporting detail the transcript shows.
    detail: transcript::TranscriptDetail,
    /// Whether the transcript follows new output by default.
    follow_tail: bool,
    _composer_subscription: Subscription,
}

impl TactApp {
    /// An offline shell: renders the whole surface but talks to no agent.
    ///
    /// Tests and the offline preview use this; [`Self::connect`] wires the real
    /// session.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(window, cx, None, Vec::new())
    }

    /// An offline shell whose sidebar lists `sessions`.
    ///
    /// The rows render exactly as they do in a connected shell, so previews and
    /// tests can cover the session list without a store or an agent.
    pub fn with_sessions(
        window: &mut Window,
        cx: &mut Context<Self>,
        sessions: Vec<RecentSession>,
    ) -> Self {
        Self::build(window, cx, None, sessions)
    }

    /// A shell seeded with prototype-shaped demo content for design review.
    ///
    /// Selected by `--preview` / `TACT_GUI_PREVIEW`. The rows, plan, diff and
    /// tasks travel through the same rendering path as live data, so what a
    /// reviewer sees here is what an agent session produces — only the source
    /// of the values differs.
    pub fn preview(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut app = Self::build(window, cx, None, preview_sessions());
        app.seed_preview(&mut *cx);
        app
    }

    /// Fill the panes with the prototype's demo task.
    fn seed_preview(&mut self, cx: &mut Context<Self>) {
        self.state.plan = preview_plan();
        self.state.tasks = preview_tasks();
        self.state.subagents = preview_subagents();
        self.state.diff = vec![
            pane::DiffEntry::new(
                "crates/tact-gui/src/shell.rs",
                "+412 -96  sidebar, transcript toolbar, session intro",
            )
            .with_stats(412, 96),
            pane::DiffEntry::new(
                "crates/tact-gui/src/pane.rs",
                "+188 -24  plan progress, diff cards, task table",
            )
            .with_stats(188, 24),
            pane::DiffEntry::new(
                "docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md",
                "+76 -32  panel headers and work footer",
            )
            .with_stats(76, 32),
        ];
        self.state.permission_mode = "ask".to_string();
        self.state.running = true;
        // The worktree group reads the live repository, so pointing the preview
        // at the source tree keeps that group honest instead of hard-coding the
        // prototype's two entries. The manifest directory is used rather than
        // the process working directory: a launcher may start the binary from
        // anywhere, and the preview must not change shape with it.
        let workdir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        self.state.branch = git_branch(&workdir);
        self.state.workdir = Some(workdir);
        self.state.background = vec!["cargo test -p tact".to_string()];
        self.state.account = Some(tact_protocol::AccountUpdate::Balance(
            tact_protocol::BalanceInfo {
                is_available: true,
                balance_infos: vec![tact_protocol::BalanceEntry {
                    currency: "USD".to_string(),
                    total_balance: 18.42,
                    granted_balance: 0.0,
                    topped_up_balance: 18.42,
                }],
            },
        ));
        self.conversation.push_user(
            "Continue with Direction A. Build a realistic 1440x900 prototype with light/dark themes, \n\
             transcript, work panes, permission state, and composer."
                .to_string(),
        );
        self.conversation.push_assistant(
            "I'll keep the transcript as the primary object and make the right pane feel like a tool for \
             verification, not a second dashboard. The first pass stays deliberately narrow: Plan, Diff, \
             Tasks, and Files.\n\n\
             - Anthropic warm neutrals with orange for the primary action.\n\
             - Compact inline tool activity; details open only when asked.\n\
             - Model, effort, permission, and context usage stay near the composer.\n\n\
             The GUI consumes `AgentUpdate` and `UserCommand`; headless crates remain free of GPU \
             dependencies."
                .to_string(),
        );
        // The prototype's transcript carries a reasoning card and inline tool
        // activity after the first answer, so the preview seeds the same shapes.
        self.conversation
            .push_row(transcript::TranscriptRow::Thinking {
                text: "The first impression should be calm and text-led. The sidebar stays \
                       persistent but subordinate; the work pane needs a restrained header."
                    .to_string(),
            });
        self.conversation
            .push_row(transcript::TranscriptRow::Tool {
                display_name: "Read".to_string(),
                detail: "crates/protocol/src/agent.rs".to_string(),
                output: "pub enum AgentUpdate {\n    StepAdded(PlanStep),\n    StepStarted { .. },\n    StepFinished { .. },\n    StepFailed { .. },\n    TaskComplete(String),\n}"
                    .to_string(),
                duration: "1.2s".to_string(),
                status: transcript::ToolStatus::Succeeded,
                expanded: false,
            });
        self.conversation
            .push_row(transcript::TranscriptRow::Tool {
                display_name: "Edited".to_string(),
                detail: "crates/tact-gui/src/theme.rs".to_string(),
                output: "+ TactTokens::light()\n+ TactTokens::dark()\n+ ThemeRegistry::watch_dir(\"themes\")"
                    .to_string(),
                duration: "0.8s".to_string(),
                status: transcript::ToolStatus::Succeeded,
                expanded: false,
            });
        self.record_change(Change::Appended(5), cx);
        cx.notify();
    }

    /// A shell connected to an agent session rooted at the process working
    /// directory, with a startup failure surfaced in the transcript.
    pub fn connect(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workdir = std::env::current_dir().ok();
        let started = tact_session::builder::init_config()
            .and_then(|_| std::env::current_dir().map_err(anyhow::Error::from))
            .and_then(session::start);
        // The list is read from the workspace's own store and never blocks the
        // window: a broken store simply shows no history.
        let recent = workdir.as_deref().map(session::recent).unwrap_or_default();

        match started {
            Ok((handle, streams)) => Self::build(window, cx, Some((handle, streams)), recent),
            Err(err) => {
                let mut app = Self::build(window, cx, None, recent);
                app.push_system_row(format!("Could not start a session: {err:#}"), cx);
                app
            }
        }
    }

    /// Shared constructor for both the offline and connected shells.
    fn build(
        window: &mut Window,
        cx: &mut Context<Self>,
        live: Option<(SessionHandle, session::SessionStreams)>,
        recent: Vec<RecentSession>,
    ) -> Self {
        let transcript_state = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("Message Tact — type @ for files or / for skills")
        });
        let session_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions"));
        let root_focus = cx.focus_handle();
        window.defer(cx, {
            let root_focus = root_focus.clone();
            move |window, cx| root_focus.focus(window, cx)
        });
        let composer_subscription =
            cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter {
                    shift: false,
                    secondary: false,
                } = event
                {
                    this.submit(window, cx);
                }
            });

        let (session, pump) = match live {
            Some((handle, streams)) => {
                let pump = Self::spawn_pump(streams, cx);
                (Some(handle), Some(pump))
            }
            None => (None, None),
        };
        let workdir = std::env::current_dir().ok();
        let branch = workdir.as_deref().and_then(git_branch);

        Self {
            workspace: Workspace::Chat,
            sidebar_open: true,
            work_pane_open: true,
            work_pane: WorkPane::default(),
            files: FilesPane::default(),
            diffs: pane::DiffPane::new(),
            conversation: Conversation::default(),
            state: SessionState {
                workdir,
                branch,
                permission_mode: "auto".to_string(),
                ..SessionState::default()
            },
            queued: VecDeque::new(),
            attachments: Vec::new(),
            transcript_state,
            composer,
            session_search,
            session,
            recent,
            _pump: pump,
            palette: None,
            root_focus,
            detail: transcript::TranscriptDetail::Normal,
            follow_tail: true,
            _composer_subscription: composer_subscription,
        }
    }

    /// Drain the session's event streams into the shell.
    ///
    /// Tokio channels are executor-agnostic, so GPUI polls them directly and
    /// the shell needs no runtime of its own. The task ends when either stream
    /// closes or the window is gone.
    fn spawn_pump(streams: session::SessionStreams, cx: &mut Context<Self>) -> session::Pump {
        let session::SessionStreams {
            mut events,
            account,
        } = streams;
        let mut account = Some(account);

        cx.spawn(async move |this, cx| {
            loop {
                let next_account = next_account(&mut account);
                tokio::select! {
                    update = events.recv() => {
                        let Some(update) = update else { break };
                        let applied = this.update(cx, |app, cx| app.apply_agent_update(update, cx));
                        if applied.is_err() {
                            break;
                        }
                    }
                    update = next_account => {
                        let Some(update) = update else { continue };
                        let applied = this.update(cx, |app, cx| app.apply_account_update(update, cx));
                        if applied.is_err() {
                            break;
                        }
                    }
                }
            }
        })
    }

    /// The session id, when a session is attached.
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_ref().map(SessionHandle::session_id)
    }

    /// Whether a turn is currently in flight.
    pub fn is_running(&self) -> bool {
        self.state.running
    }

    /// Number of transcript rows, for tests and status surfaces.
    pub fn transcript_len(&self) -> usize {
        self.conversation.len()
    }

    /// Open the Tact command palette over the current window.
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        self.palette = Some(state.clone());
        let owner = cx.weak_entity();
        let dialog_state = state.clone();

        window.open_dialog(cx, move |dialog, _window, _cx| {
            let owner = owner.clone();
            let dialog_state = dialog_state.clone();
            dialog
                .title("Command palette")
                .width(px(560.))
                .content(move |content, _window, _cx| {
                    let owner = owner.clone();
                    content.child(
                        commands::groups().into_iter().fold(
                            Command::new(&dialog_state)
                                .placeholder("Type a command…")
                                .on_confirm(move |index, window, cx| {
                                    if let Some(command) = PaletteCommand::from_index(index) {
                                        let _ = owner.update(cx, |app, cx| {
                                            app.run_palette_command(command, window, cx)
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                            |command, group| command.group(group),
                        ),
                    )
                })
        });

        window.defer(cx, move |window, cx| {
            state.update(cx, |state, cx| state.focus(window, cx));
        });
    }

    /// Execute one palette command.
    fn run_palette_command(
        &mut self,
        command: PaletteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            PaletteCommand::OpenPalette => self.open_palette(window, cx),
            PaletteCommand::ToggleSidebar => {
                self.sidebar_open = !self.sidebar_open;
                cx.notify();
            }
            PaletteCommand::ToggleWorkPane => {
                self.work_pane_open = !self.work_pane_open;
                cx.notify();
            }
            PaletteCommand::OpenDiff => {
                self.workspace = Workspace::Code;
                self.work_pane = WorkPane::Diff;
                self.work_pane_open = true;
                cx.notify();
            }
            PaletteCommand::OpenTasks => {
                self.workspace = Workspace::Agent;
                self.work_pane = WorkPane::Tasks;
                self.work_pane_open = true;
                cx.notify();
            }
            PaletteCommand::NewSession => self.new_session(cx),
            PaletteCommand::FocusComposer => self.focus_composer(window, cx),
            PaletteCommand::StopTask => self.stop(cx),
            PaletteCommand::CycleTranscriptDetail => self.cycle_detail(cx),
            PaletteCommand::CycleSessions => match self.next_session_id() {
                Some(session_id) => self.resume_session(session_id, cx),
                None => self.push_system_row(
                    "No other session in this workspace to switch to.".into(),
                    cx,
                ),
            },
            PaletteCommand::CycleSessionsBackward => match self.previous_session_id() {
                Some(session_id) => self.resume_session(session_id, cx),
                None => self.push_system_row(
                    "No other session in this workspace to switch to.".into(),
                    cx,
                ),
            },
            PaletteCommand::CompactSession => {
                self.send_command(tact_protocol::UserCommand::Compact, cx)
            }
            PaletteCommand::SessionStats => {
                self.send_command(tact_protocol::UserCommand::QueryStats, cx)
            }
            PaletteCommand::McpServers => {
                self.send_command(tact_protocol::UserCommand::McpList, cx)
            }
            PaletteCommand::OpenSettings => self.open_settings(window, cx),
            PaletteCommand::ToggleTheme => theme::toggle(window, cx),
        }
    }

    /// Start a fresh agent session in the current workspace.
    fn new_session(&mut self, cx: &mut Context<Self>) {
        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };

        match session::start(workdir.clone()) {
            Ok((handle, streams)) => self.adopt(workdir, handle, streams, cx),
            Err(err) => self.push_system_row(format!("Could not start a session: {err:#}"), cx),
        }
    }

    /// Reopen a session from the sidebar.
    ///
    /// Resume is id reuse: the agent reloads that session's stored turns while
    /// it is constructed, so the reopened session continues the same thread
    /// even though the transcript starts on a fresh page.
    fn resume_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        if self.session.as_ref().map(SessionHandle::session_id) == Some(session_id.as_str()) {
            return;
        }
        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };

        match session::resume(workdir.clone(), session_id.clone()) {
            Ok((handle, streams)) => {
                let short = session::short_id(&session_id).to_string();
                self.adopt(workdir, handle, streams, cx);
                self.push_system_row(
                    format!("Resumed session {short} — the agent keeps its earlier turns."),
                    cx,
                );
            }
            Err(err) => self.push_system_row(
                format!("Could not resume session {session_id}: {err:#}"),
                cx,
            ),
        }
    }

    /// The session after the current one in sidebar order, wrapping at the end.
    ///
    /// This is what makes the keyboard contract complete: `Ctrl+Tab` reaches
    /// every session in the workspace without a pointer.
    fn next_session_id(&self) -> Option<String> {
        if self.recent.is_empty() {
            return None;
        }
        let current = self.session.as_ref().map(SessionHandle::session_id);
        let position = current.and_then(|id| self.recent.iter().position(|row| row.id == id));
        let next = next_session_index(self.recent.len(), position)?;
        self.recent.get(next).map(|row| row.id.clone())
    }

    /// The session before the current one in sidebar order, wrapping at the start.
    fn previous_session_id(&self) -> Option<String> {
        if self.recent.is_empty() {
            return None;
        }
        let current = self.session.as_ref().map(SessionHandle::session_id);
        let position = current.and_then(|id| self.recent.iter().position(|row| row.id == id));
        let previous = previous_session_index(self.recent.len(), position)?;
        self.recent.get(previous).map(|row| row.id.clone())
    }

    /// The directory this window works in.
    fn workspace_dir(&self) -> Option<PathBuf> {
        self.state
            .workdir
            .clone()
            .or_else(|| std::env::current_dir().ok())
    }

    /// Adopt a started session, discarding everything the previous one owned.
    fn adopt(
        &mut self,
        workdir: PathBuf,
        handle: SessionHandle,
        streams: session::SessionStreams,
        cx: &mut Context<Self>,
    ) {
        self.session = Some(handle);
        self._pump = Some(Self::spawn_pump(streams, cx));
        self.conversation = Conversation::default();
        self.diffs.invalidate();
        self.state = SessionState {
            workdir: Some(workdir.clone()),
            branch: git_branch(&workdir),
            permission_mode: "auto".to_string(),
            ..SessionState::default()
        };
        self.queued.clear();
        self.attachments.clear();
        self.transcript_state
            .update(cx, |state, cx| state.reset(0, cx));
        self.recent = session::recent(&workdir);
        cx.notify();
    }

    /// Advance the transcript through Normal → Thinking → Verbose.
    fn cycle_detail(&mut self, cx: &mut Context<Self>) {
        self.detail = self.detail.next();
        self.transcript_state
            .update(cx, |state, cx| state.remeasure(cx));
        cx.notify();
    }

    /// Focus the message composer.
    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |state, cx| state.focus_handle(cx).focus(window, cx));
    }

    /// Ask the platform for files and add the selected paths as chips.
    fn attach_files(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach to message".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                for path in paths {
                    app.add_attachment(path);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Add one attachment chip, de-duplicating by path.
    fn add_attachment(&mut self, path: PathBuf) {
        if self
            .attachments
            .iter()
            .any(|attachment| attachment.path == path)
        {
            return;
        }
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        self.attachments.push(Attachment { label, path });
    }

    /// Remove one attachment chip.
    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            cx.notify();
        }
    }

    /// Keyboard removal for the most recent attachment chip.
    fn remove_last_attachment(&mut self, cx: &mut Context<Self>) {
        if self.attachments.pop().is_some() {
            cx.notify();
        }
    }

    /// Apply a file or skill completion selected from the inline suggestion list.
    fn accept_suggestion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.composer.read(cx).value().to_string();
        let Some(trigger) = composer::parse_trigger(&draft) else {
            return;
        };
        let Some(suggestion) = composer::suggestions(&draft, self.workspace_dir().as_deref())
            .get(index)
            .cloned()
        else {
            return;
        };
        if suggestion.kind == composer::SuggestionKind::File {
            let path = suggestion
                .insertion
                .strip_prefix('@')
                .map(PathBuf::from)
                .map(|path| {
                    if path.is_relative() {
                        self.workspace_dir()
                            .map(|root| root.join(&path))
                            .unwrap_or(path)
                    } else {
                        path
                    }
                });
            if let Some(path) = path {
                self.add_attachment(path);
            }
        }
        let next = composer::apply_suggestion(&draft, &trigger, &suggestion.insertion);
        self.composer
            .update(cx, |state, cx| state.set_value(next, window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Open the slash-command completion list without requiring a keyboard.
    fn open_skill_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |state, cx| state.set_value("/", window, cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Switch the active model and reflect the request immediately; `ModelInfo`
    /// remains the authoritative acknowledgement when the agent emits it.
    fn set_model(&mut self, model: String, cx: &mut Context<Self>) {
        if let Some(params) = self.state.model.as_mut() {
            params.model = model.clone();
        } else {
            self.state.model = Some(tact_protocol::ModelCallParams {
                model: model.clone(),
                max_tokens: 0,
                thinking_budget: None,
                reasoning_effort: None,
                extra_body: None,
            });
        }
        self.send_command(tact_protocol::UserCommand::SetModel(model), cx);
    }

    /// Set or clear the reasoning effort for subsequent LLM requests.
    fn set_reasoning_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        if let Some(params) = self.state.model.as_mut() {
            params.reasoning_effort = effort.clone();
        }
        self.send_command(tact_protocol::UserCommand::SetReasoningEffort(effort), cx);
    }

    /// Set the thinking budget for subsequent LLM requests.
    fn set_thinking_budget(&mut self, budget: usize, cx: &mut Context<Self>) {
        if let Some(params) = self.state.model.as_mut() {
            params.thinking_budget = Some(budget as u32);
        }
        self.send_command(tact_protocol::UserCommand::SetThinkingBudget(budget), cx);
    }

    /// Set the in-memory permission mode for the active session.
    fn set_permission_mode(&mut self, mode: String, cx: &mut Context<Self>) {
        self.state.permission_mode = mode.clone();
        self.send_command(tact_protocol::UserCommand::SetPermissionMode(mode), cx);
    }

    /// Open the Tact settings dialog.
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let owner = owner.clone();
            dialog
                .title("Settings")
                .width(px(720.))
                .content(move |content, window, cx| {
                    content.child(settings_panel(owner.clone(), window, cx))
                })
        });
    }

    fn on_open_palette(
        &mut self,
        _: &commands::OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette(window, cx);
    }

    fn on_new_session(
        &mut self,
        _: &commands::NewSession,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_session(cx);
    }

    fn on_stop_task(
        &mut self,
        _: &commands::StopTask,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop(cx);
    }

    fn on_toggle_work_pane(
        &mut self,
        _: &commands::ToggleWorkPane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.work_pane_open = !self.work_pane_open;
        cx.notify();
    }

    fn on_focus_composer(
        &mut self,
        _: &commands::FocusComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_composer(window, cx);
    }

    fn on_cycle_detail(
        &mut self,
        _: &commands::CycleTranscriptDetail,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::CycleTranscriptDetail, _window, cx);
    }

    fn on_toggle_sidebar(
        &mut self,
        _: &commands::ToggleSidebar,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    fn on_open_diff(
        &mut self,
        _: &commands::OpenDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::OpenDiff, window, cx);
    }

    fn on_open_tasks(
        &mut self,
        _: &commands::OpenTasks,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::OpenTasks, window, cx);
    }

    fn on_open_settings(
        &mut self,
        _: &commands::OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(window, cx);
    }

    fn on_cycle_sessions(
        &mut self,
        _: &commands::CycleSessions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::CycleSessions, window, cx);
    }

    fn on_cycle_sessions_backward(
        &mut self,
        _: &commands::CycleSessionsBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::CycleSessionsBackward, window, cx);
    }

    fn on_remove_attachment(
        &mut self,
        _: &commands::RemoveAttachment,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remove_last_attachment(cx);
    }

    fn on_compact_session(
        &mut self,
        _: &commands::CompactSession,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_command(tact_protocol::UserCommand::Compact, cx);
    }

    fn on_session_stats(
        &mut self,
        _: &commands::SessionStats,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_command(tact_protocol::UserCommand::QueryStats, cx);
    }

    fn on_mcp_servers(
        &mut self,
        _: &commands::McpServers,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_command(tact_protocol::UserCommand::McpList, cx);
    }

    fn on_toggle_theme(
        &mut self,
        _: &commands::ToggleTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme::toggle(window, cx);
    }

    /// Select a work pane tab.
    pub(crate) fn set_work_pane(&mut self, pane: WorkPane) {
        self.work_pane = pane;
    }

    /// Expand or collapse a directory in the Files pane.
    pub(crate) fn toggle_directory(&mut self, path: std::path::PathBuf) {
        self.files.on_toggle(path);
    }

    /// Append a local notice to the transcript without an agent session.
    ///
    /// The offline shell uses this for startup messages, and tests use it to
    /// populate the transcript without standing up a provider.
    pub fn push_notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.push_system_row(text.into(), cx);
    }

    /// Submit the composer draft, queueing it behind an in-flight turn.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.composer.read(cx).value().trim().to_string();
        if draft.is_empty() {
            return;
        }

        let submitted = composer::with_attachments(&draft, &self.attachments);
        let display = if self.attachments.is_empty() {
            draft
        } else {
            format!("{}\n\nAttached: {}", draft, self.attachments.len())
        };
        self.conversation.push_user(display);
        self.record_change(Change::Appended(1), cx);
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.attachments.clear();

        // `SubmitTask` blocks the driver until the in-flight turn finishes, so
        // queueing here keeps Stop responsive instead of stalling the command
        // loop. The queue drains as soon as the turn ends.
        if self.state.running {
            self.queued.push_back(submitted);
            self.push_system_row(
                "Queued — sends when the current turn finishes.".to_string(),
                cx,
            );
            return;
        }

        self.dispatch(submitted, cx);
        cx.notify();
    }

    /// Hand a prompt to the session and enter the running state.
    fn dispatch(&mut self, prompt: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref() else {
            self.push_system_row(
                "No agent session is attached; the message was not sent.".to_string(),
                cx,
            );
            return;
        };
        if !session.submit(prompt) {
            self.push_system_row("The agent session has ended.".to_string(), cx);
            return;
        }
        self.state.running = true;
    }

    /// Stop the active turn without discarding the draft.
    fn stop(&mut self, _cx: &mut Context<Self>) {
        if !self.state.running {
            return;
        }
        // `TaskCancelled` arrives once the loop leaves the turn; that update
        // appends the system row and clears `running`, so nothing is recorded
        // here that the agent could contradict.
        if let Some(session) = self.session.as_ref() {
            session.cancel();
        }
    }

    /// Send the next queued prompt, if any.
    fn flush_queue(&mut self, cx: &mut Context<Self>) {
        if self.session.is_none() {
            return;
        }
        if let Some(next) = self.queued.pop_front() {
            self.dispatch(next, cx);
        }
    }

    /// Fold one agent update into the transcript and session state.
    fn apply_agent_update(&mut self, update: tact_protocol::AgentUpdate, cx: &mut Context<Self>) {
        let ends_turn = matches!(
            update,
            tact_protocol::AgentUpdate::TaskComplete(_)
                | tact_protocol::AgentUpdate::TaskCancelled
                | tact_protocol::AgentUpdate::Error(_)
        );
        let before = self.state.diff.len();
        let change = self.conversation.apply(update, &mut self.state);
        // A newly recorded file change moves the working tree on from whatever
        // the Diff pane cached, so its bodies are re-read on the next frame.
        if self.state.diff.len() != before {
            self.diffs.invalidate();
        }
        self.record_change(change, cx);
        if ends_turn {
            self.flush_queue(cx);
        }
        cx.notify();
    }

    /// Fold one account update into the session state.
    fn apply_account_update(
        &mut self,
        update: tact_protocol::AccountUpdate,
        cx: &mut Context<Self>,
    ) {
        self.state.account = Some(update);
        cx.notify();
    }

    /// Apply a scroller change emitted by the conversation.
    /// Open or close one collapsible transcript row.
    fn toggle_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.conversation.toggle_expanded(index) {
            self.record_change(Change::Resized(index), cx);
        }
        cx.notify();
    }

    fn record_change(&mut self, change: Change, cx: &mut Context<Self>) {
        match change {
            Change::None => {}
            Change::Appended(count) => {
                self.transcript_state
                    .update(cx, |state, cx| state.append(count, cx));
            }
            Change::Resized(index) => {
                self.transcript_state
                    .update(cx, |state, cx| state.remeasure_items(index..index + 1, cx));
            }
        }
        if self.follow_tail {
            self.transcript_state
                .update(cx, |state, cx| state.scroll_to_end(cx));
        }
    }

    /// Append a local system row (startup and session-level notices).
    fn push_system_row(&mut self, text: String, cx: &mut Context<Self>) {
        self.conversation.push_system(text);
        self.record_change(Change::Appended(1), cx);
        cx.notify();
    }

    /// Answer a single-choice request.
    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(request) = self.state.request.clone() else {
            return;
        };
        if request.multi {
            if let Some(request) = self.state.request.as_mut() {
                match request
                    .selected
                    .iter()
                    .position(|selected| *selected == index)
                {
                    Some(position) => {
                        request.selected.remove(position);
                    }
                    None => request.selected.push(index),
                }
            }
            cx.notify();
            return;
        }
        self.answer(
            UiResponse::Select {
                request_id: request.id,
                choice: Some(index),
            },
            cx,
        );
    }

    /// Confirm a multi-select request.
    fn confirm_multi(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.state.request.clone() else {
            return;
        };
        self.answer(
            UiResponse::MultiSelect {
                request_id: request.id,
                choices: Some(request.selected.clone()),
            },
            cx,
        );
    }

    /// Dismiss the pending request without choosing.
    fn cancel_request(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.state.request.clone() else {
            return;
        };
        let response = if request.multi {
            UiResponse::MultiSelect {
                request_id: request.id,
                choices: None,
            }
        } else {
            UiResponse::Select {
                request_id: request.id,
                choice: None,
            }
        };
        self.answer(response, cx);
    }

    /// Send a response and clear the prompt so it cannot be answered twice.
    fn answer(&mut self, response: UiResponse, cx: &mut Context<Self>) {
        if let Some(session) = self.session.as_ref() {
            session.send(tact_protocol::UserCommand::UiResponse(response));
        }
        self.state.request = None;
        cx.notify();
    }

    /// Send a non-task command to the attached session.
    fn send_command(&mut self, command: tact_protocol::UserCommand, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref() else {
            self.push_system_row(
                "No agent session is attached; the command was not sent.".to_string(),
                cx,
            );
            return;
        };
        session.send(command);
        cx.notify();
    }
}

impl Render for TactApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = window.bounds().size.width;
        let rem_size = window.rem_size();
        let sidebar_is_overlay = width < SIDEBAR_OVERLAY_UNDER.to_pixels(rem_size);
        let work_pane_in_flow =
            self.work_pane_open && width >= WORK_PANE_IN_FLOW_FROM.to_pixels(rem_size);
        let work_pane_is_drawer = self.work_pane_open && !work_pane_in_flow;

        let session_heading = SharedString::from(format!(
            "{} · {} workspace",
            project_label(&self.state),
            self.workspace.label()
        ));
        let transcript_header = TranscriptHeader {
            heading: session_heading.clone(),
            detail: self.detail,
        };

        let mut workspace_row = h_flex().size_full().min_h_0().items_stretch();
        if !sidebar_is_overlay && self.sidebar_open {
            workspace_row = workspace_row.child(sidebar(
                &self.state,
                &self.recent,
                self.session.as_ref().map(SessionHandle::session_id),
                &self.session_search,
                cx,
            ));
        }
        workspace_row = workspace_row.child(transcript(
            self.conversation.rows(),
            self.transcript_state.clone(),
            self.composer.clone(),
            &self.state,
            &self.attachments,
            transcript_header,
            cx,
        ));

        if work_pane_in_flow {
            workspace_row = workspace_row.child(work_pane(
                self.work_pane,
                &self.state,
                &self.files,
                &mut self.diffs,
                cx,
            ));
        }

        let mut workspace = div().relative().flex_1().min_h_0().child(workspace_row);
        if sidebar_is_overlay && self.sidebar_open {
            workspace = workspace.child(sidebar_overlay(
                &self.state,
                &self.recent,
                self.session.as_ref().map(SessionHandle::session_id),
                &self.session_search,
                cx,
            ));
        }
        if work_pane_is_drawer {
            workspace = workspace.child(work_pane_drawer(
                self.work_pane,
                &self.state,
                &self.files,
                &mut self.diffs,
                cx,
            ));
        }

        v_flex()
            .id("tact-app")
            .test_support()
            .size_full()
            .track_focus(&self.root_focus)
            .key_context(commands::CONTEXT)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_open_palette))
            .on_action(cx.listener(Self::on_new_session))
            .on_action(cx.listener(Self::on_stop_task))
            .on_action(cx.listener(Self::on_toggle_work_pane))
            .on_action(cx.listener(Self::on_focus_composer))
            .on_action(cx.listener(Self::on_cycle_detail))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_open_diff))
            .on_action(cx.listener(Self::on_open_tasks))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_cycle_sessions))
            .on_action(cx.listener(Self::on_cycle_sessions_backward))
            .on_action(cx.listener(Self::on_remove_attachment))
            .on_action(cx.listener(Self::on_compact_session))
            .on_action(cx.listener(Self::on_session_stats))
            .on_action(cx.listener(Self::on_mcp_servers))
            .on_action(cx.listener(Self::on_toggle_theme))
            .child(title_bar(
                self.workspace,
                self.sidebar_open,
                self.work_pane_open,
                session_heading,
                width < px(1320.),
                cx,
            ))
            .child(workspace)
            .child(status_bar(self.workspace, &self.state, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

/// Workspace name shown in the title bar, sidebar footer, and transcript intro.
fn project_label(state: &SessionState) -> String {
    state
        .workdir
        .as_deref()
        .and_then(std::path::Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("Workspace")
        .to_string()
}

/// Await the account stream, parking forever once it has closed.
///
/// The branch stays in the `select!` but can never resolve again, which is what
/// keeps an unsupported provider from spinning the pump.
async fn next_account(
    slot: &mut Option<tokio::sync::mpsc::UnboundedReceiver<tact_protocol::AccountUpdate>>,
) -> Option<tact_protocol::AccountUpdate> {
    let Some(receiver) = slot.as_mut() else {
        return std::future::pending().await;
    };
    let update = receiver.recv().await;
    if update.is_none() {
        *slot = None;
    }
    update
}

fn title_bar(
    workspace: Workspace,
    sidebar_open: bool,
    work_pane_open: bool,
    session_heading: SharedString,
    compact: bool,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let dark = cx.theme().is_dark();
    let (theme_icon, theme_action) = if dark {
        (IconName::Sun, "Switch to light theme")
    } else {
        (IconName::Moon, "Switch to dark theme")
    };
    let (pane_icon, pane_action) = if work_pane_open {
        (IconName::PanelRightClose, "Hide work pane")
    } else {
        (IconName::PanelRightOpen, "Show work pane")
    };
    let (sidebar_icon, sidebar_action) = if sidebar_open {
        (IconName::PanelLeftClose, "Hide sidebar")
    } else {
        (IconName::PanelLeftOpen, "Show sidebar")
    };

    let tabs = TabBar::new("workspace-tabs")
        .segmented()
        .selected_index(workspace.index())
        .children(Workspace::ALL.map(|preset| Tab::new().label(preset.label())))
        .on_click(cx.listener(|this, index, _, cx| {
            this.workspace = Workspace::from_index(*index);
            cx.notify();
        }));

    // The prototype's session chip: a list glyph, the name, then a chevron.
    let session_label = h_flex()
        .min_w_0()
        .max_w(rems(17.))
        .items_center()
        .gap_1()
        .px_1()
        .py_0p5()
        .rounded(cx.theme().radius)
        .text_color(cx.theme().muted_foreground)
        .child(IconName::MessageSquareText)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(session_heading),
        )
        .child(IconName::ChevronDown);

    let mut command = Button::new("open-command-palette")
        .icon(IconName::Search)
        .tooltip("Command palette (Ctrl+K)")
        .accessibility_label("Open command palette")
        .ghost()
        .compact()
        .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)));
    if !compact {
        command = command.label("Search or run a command");
    }

    let sidebar_toggle = Button::new("toggle-sidebar")
        .icon(sidebar_icon)
        .tooltip(sidebar_action)
        .accessibility_label(sidebar_action)
        .toggled(sidebar_open)
        .ghost()
        .compact()
        .on_click(cx.listener(|this, _, _, cx| {
            this.sidebar_open = !this.sidebar_open;
            cx.notify();
        }));

    let controls = h_flex()
        .items_center()
        .gap_1()
        .child(command)
        .child(
            Button::new("open-settings")
                .icon(IconName::Settings)
                .tooltip("Settings (Ctrl+,)")
                .accessibility_label("Open settings")
                .ghost()
                .compact()
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
        )
        .child(
            Button::new("toggle-theme")
                .icon(theme_icon)
                .tooltip(theme_action)
                .accessibility_label(theme_action)
                .ghost()
                .compact()
                .on_click(cx.listener(|_, _, window, cx| theme::toggle(window, cx))),
        )
        .child(
            Button::new("toggle-work-pane")
                .icon(pane_icon)
                .tooltip(pane_action)
                .accessibility_label(pane_action)
                .toggled(work_pane_open)
                .ghost()
                .compact()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.work_pane_open = !this.work_pane_open;
                    cx.notify();
                })),
        );

    TitleBar::new().h(TITLE_BAR_HEIGHT).child(
        h_flex()
            .h_full()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(sidebar_toggle)
                    .child(
                        // Brand mark, matching the prototype's dark "T" tile.
                        div()
                            .size(rems(1.5))
                            .flex_shrink_0()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().foreground)
                            .text_color(cx.theme().background)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .child(SharedString::from("T")),
                    )
                    .child(tabs)
                    .child(session_label),
            )
            .child(div().flex_1())
            .child(controls),
    )
}

/// Demo sessions for the design preview, shaped like the prototype's sidebar.
fn preview_sessions() -> Vec<RecentSession> {
    let now = session::now_unix();
    // Ages bucket into the prototype's 3 / 5 split: under a day reads as Today,
    // the rest fall inside the week.
    let rows = [
        ("7fbab10-2c41-4c9a-9f10-2222aaaa1111", 60, 12),
        ("3b78ba4-1d02-4a33-8b71-3333bbbb2222", 2_700, 4),
        ("d464f22d-5e11-4c2f-9a08-4444cccc3333", 6_000, 7),
        ("daf05fa-71b4-4d0e-8e55-5555dddd4444", 90_000, 2),
        ("6278b7fa-3c92-4f18-9b27-6666eeee5555", 100_000, 5),
        ("306ea551-8a13-4b62-8f04-7777ffff6666", 130_000, 3),
        ("036e6015-a4d7-4e29-8c31-8888aaaa7777", 160_000, 9),
        ("3f016556-2b8c-4f70-9d12-9999bbbb8888", 200_000, 1),
    ];

    rows.iter()
        .map(|(id, age, messages)| RecentSession {
            id: (*id).to_string(),
            updated_at_unix: now - age,
            message_count: *messages,
        })
        .collect()
}

/// Demo plan steps for the design preview.
fn preview_plan() -> Vec<tact_protocol::PlanStep> {
    let mut steps = Vec::new();
    let plan = [
        (
            "Lock the design direction",
            "Continuous Workspace with one work pane",
            Some("Direction A approved; composable workspace with one work pane."),
        ),
        (
            "Normalize Anthropic tokens",
            "Light and dark theme registry",
            Some("Light and dark palettes are byte-identical to the design JSON."),
        ),
        (
            "Build interactive shell prototype",
            "Title bar, sidebar, transcript, work pane",
            None,
        ),
        (
            "Wire protocol events",
            "AgentUpdate and UserCommand mapping",
            None,
        ),
        (
            "Run design review",
            "Keyboard, focus, resize, contrast",
            None,
        ),
    ];

    for (description, tool, output) in plan {
        let mut step = tact_protocol::PlanStep::new(
            description,
            tool,
            format!("preview-{tool}"),
            Vec::<(String, String)>::new(),
        );
        step.output = output.map(str::to_string);
        steps.push(step);
    }

    steps
}

/// Demo tasks for the design preview.
fn preview_tasks() -> Vec<tact_protocol::TaskSnapshot> {
    use tact_protocol::TaskStatusSnapshot;

    let rows = [
        (1, "Lock the design direction", TaskStatusSnapshot::Completed),
        (
            2,
            "Normalize Anthropic tokens",
            TaskStatusSnapshot::Completed,
        ),
        (
            3,
            "Build interactive shell prototype",
            TaskStatusSnapshot::InProgress,
        ),
        (4, "Run design review", TaskStatusSnapshot::Pending),
    ];

    rows.into_iter()
        .map(|(id, subject, status)| tact_protocol::TaskSnapshot {
            id,
            subject: subject.to_string(),
            status,
            session_id: "preview".to_string(),
            owner: "tact".to_string(),
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            created_at: None,
            started_at: None,
            completed_at: None,
        })
        .collect()
}

/// Demo subagent runs for the design preview.
fn preview_subagents() -> Vec<tact_protocol::SubagentRunSnapshot> {
    use tact_protocol::SubagentStatusSnapshot;

    vec![
        tact_protocol::SubagentRunSnapshot {
            child_id: "preview-review".to_string(),
            status: SubagentStatusSnapshot::Running,
            summary_first: "Auditing focus, resize, and contrast".to_string(),
            started_at: None,
            finished_at: None,
        },
        tact_protocol::SubagentRunSnapshot {
            child_id: "preview-tokens".to_string(),
            status: SubagentStatusSnapshot::Completed,
            summary_first: "Token parity verified".to_string(),
            started_at: None,
            finished_at: None,
        },
    ]
}

fn sidebar(
    state: &SessionState,
    recent: &[RecentSession],
    current: Option<&str>,
    search: &Entity<InputState>,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let session_line = session_activity_line(state);
    // Bound once: hover and highlight closures must not borrow the context.
    let radius = cx.theme().radius;
    let hover_bg = cx.theme().sidebar_accent;
    let current_bg = cx.theme().sidebar_accent;
    let muted = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let now = session::now_unix();
    let query = search.read(cx).value().to_lowercase();
    let project = project_label(state);
    let branch = state.branch.as_deref();

    let mut list = v_flex().gap_4().px_2().pb_3();
    for bucket in session_buckets(recent, &query, now) {
        let mut rows = v_flex().gap_1();
        for session in bucket.sessions {
            let is_current = current == Some(session.id.as_str());
            let session_id = session.id.clone();
            rows = rows.child(session_row(
                &session,
                is_current,
                &project,
                branch,
                radius,
                hover_bg,
                current_bg,
                muted,
                primary,
                cx,
                move |this, cx| this.resume_session(session_id.clone(), cx),
            ));
        }

        list = list.child(
            v_flex().gap_1().child(group_label(&bucket.label, bucket.count, cx)).child(rows),
        );
    }

    if query.is_empty() && recent.is_empty() {
        list = list.child(
            v_flex()
                .px_2()
                .py_3()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from("No sessions yet")),
        );
    }

    // The prototype closes the sidebar with the workspace's git worktrees and
    // the live background work. Both are real: worktrees come from `git
    // worktree list`, background rows from keep-live tool cards that have not
    // finalized yet. An empty group is dropped rather than shown as a stub.
    if query.is_empty() {
        let worktrees = worktree_rows(state);
        if !worktrees.is_empty() {
            let mut rows = v_flex().gap_1();
            for worktree in &worktrees {
                rows = rows.child(worktree_row(
                    worktree, radius, hover_bg, muted, primary, cx,
                ));
            }
            list = list.child(
                v_flex()
                    .gap_1()
                    .child(group_label("Worktrees", worktrees.len(), cx))
                    .child(rows),
            );
        }

        let background = background_rows(state);
        if !background.is_empty() {
            let mut rows = v_flex().gap_1();
            for task in &background {
                rows = rows.child(background_row(
                    task, radius, hover_bg, muted, primary, cx,
                ));
            }
            list = list.child(
                v_flex()
                    .gap_1()
                    .child(group_label("Background", background.len(), cx))
                    .child(rows),
            );
        }
    }

    v_flex()
        .flex_shrink_0()
        .w(SIDEBAR_WIDTH)
        .h_full()
        .border_r_1()
        .border_color(cx.theme().sidebar_border)
        .bg(cx.theme().sidebar)
        .text_color(cx.theme().sidebar_foreground)
        .child(sidebar_top(search, cx))
        .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(list))
        .child(sidebar_footer(
            &project_label(state),
            state.branch.as_deref(),
            &session_line,
            cx,
        ))
        .id("sidebar")
        .test_support()
}

/// The sidebar's fixed header: the new-session action and the session filter.
///
/// Mirrors the prototype's `sideTop` block, which keeps both controls pinned
/// above the scrolling session list.
fn sidebar_top(search: &Entity<InputState>, cx: &mut Context<TactApp>) -> impl IntoElement {
    let radius = cx.theme().radius;
    let hover_bg = cx.theme().sidebar_accent;
    let primary = cx.theme().primary;

    v_flex()
        .flex_shrink_0()
        .gap_2()
        .px_2()
        .pt_2()
        .pb_2()
        .child(
            h_flex()
                .id("session-new")
                .test_support()
                .items_center()
                .gap_2()
                .border_1()
                .border_color(cx.theme().sidebar_border)
                .bg(cx.theme().popover)
                .px_2()
                .py_2()
                .rounded(radius)
                .text_sm()
                .text_color(primary)
                .hover(move |style| style.bg(hover_bg))
                .on_click(cx.listener(|this, _, _, cx| this.new_session(cx)))
                .child(IconName::Plus)
                .child(SharedString::from("New session")),
        )
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .border_1()
                .border_color(cx.theme().sidebar_border.opacity(0.6))
                .bg(cx.theme().background)
                .px_2()
                .py_1()
                .rounded(radius)
                .text_color(cx.theme().muted_foreground)
                .child(IconName::Search)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .child(Input::new(search)),
                ),
        )
        .id("sidebar-top")
        .test_support()
}

/// One sidebar session row: status dot, title, and a metadata badge.
#[allow(clippy::too_many_arguments)]
fn session_row(
    session: &RecentSession,
    is_current: bool,
    project: &str,
    branch: Option<&str>,
    radius: gpui_kit::gpui::Pixels,
    hover_bg: gpui_kit::gpui::Hsla,
    current_bg: gpui_kit::gpui::Hsla,
    muted: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    cx: &mut Context<TactApp>,
    on_click: impl Fn(&mut TactApp, &mut Context<TactApp>) + 'static,
) -> impl IntoElement {
    let label = session_row_title(session);
    // The prototype's metadata line reads `<project> · <branch>`; the age is
    // what makes two rows of the same workspace distinguishable.
    let age = session::age_label(session::now_unix().saturating_sub(session.updated_at_unix));
    let meta = match branch {
        Some(branch) => format!("{project} · {branch} · {age}"),
        None => format!("{project} · {age}"),
    };
    let (dot, dot_bg) = if is_current {
        (primary, primary.opacity(0.16))
    } else {
        (muted.opacity(0.5), muted.opacity(0.12))
    };
    let badge = if is_current {
        Some(("Running", primary, primary.opacity(0.14)))
    } else if session.message_count > 0 {
        Some((
            "Review",
            muted,
            muted.opacity(0.10),
        ))
    } else {
        None
    };

    h_flex()
        .id(SharedString::from(format!("session-row-{}", session.id)))
        .test_support()
        .items_start()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(radius)
        .when(is_current, move |row| row.bg(current_bg))
        .hover(move |style| style.bg(hover_bg))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(
            div()
                .mt_1()
                .size(rems(0.4375))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .border_1()
                .border_color(dot_bg),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .when(is_current, |title| title.text_color(primary))
                        .child(SharedString::from(label)),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(SharedString::from(meta)),
                        )
                        .when_some(badge, |row, (text, fg, bg)| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .rounded(cx.theme().radius)
                                    .bg(bg)
                                    .px_1()
                                    .text_xs()
                                    .text_color(fg)
                                    .child(SharedString::from(text)),
                            )
                        }),
                ),
        )
}

/// Sidebar row title: the session's short id, which is what a store records.
fn session_row_title(session: &RecentSession) -> String {
    session::short_id(&session.id).to_string()
}

/// One entry of `git worktree list`, trimmed to what the sidebar shows.
struct WorktreeRow {
    /// Branch name, or the directory's file name when detached.
    name: String,
    /// The worktree's own directory name, used as the description line.
    detail: String,
    /// Whether this is the worktree the current session is rooted in.
    is_current: bool,
}

/// The workspace's git worktrees, current first.
///
/// Shelling out to git costs a process per render, so the sidebar only calls
/// this for an empty query and an attached workspace; a non-repository simply
/// yields nothing and the group disappears.
fn worktree_rows(state: &SessionState) -> Vec<WorktreeRow> {
    let Some(workdir) = state.workdir.as_deref() else {
        return Vec::new();
    };
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok();
    let Some(output) = output.filter(|output| output.status.success()) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);

    let mut rows = Vec::new();
    for block in text.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut path: Option<&str> = None;
        let mut branch: Option<&str> = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(rest.trim());
            } else if let Some(rest) = line.strip_prefix("branch ") {
                branch = Some(rest.trim().trim_start_matches("refs/heads/"));
            }
        }
        let Some(path) = path else { continue };
        let path = std::path::Path::new(path);
        // A session can be rooted at the worktree root or in a subdirectory of
        // it (and either side may be reached through a symlink), so compare
        // canonical paths and accept containment rather than equality.
        let canonical = |candidate: &std::path::Path| {
            candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.to_path_buf())
        };
        let workdir = canonical(workdir);
        let path_canonical = canonical(path);
        let is_current = workdir.starts_with(&path_canonical);
        let detail = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("worktree")
            .to_string();
        let name = branch.map(str::to_string).unwrap_or_else(|| detail.clone());
        rows.push(WorktreeRow {
            name,
            detail,
            is_current,
        });
    }

    // The session's own worktree leads; the rest keep git's listing order.
    rows.sort_by_key(|row| !row.is_current);
    rows
}

/// A background command that is still running.
struct BackgroundRow {
    /// The command as the agent named it.
    command: String,
}

/// Live background commands, in the order they started.
///
/// The session records a command when a keep-live tool card opens and removes
/// it once the task finalizes, so this group reflects real state rather than a
/// placeholder.
fn background_rows(state: &SessionState) -> Vec<BackgroundRow> {
    state
        .background
        .iter()
        .map(|command| BackgroundRow {
            command: command.clone(),
        })
        .collect()
}

/// A worktree row: branch, its directory, and the current-worktree marker.
fn worktree_row(
    worktree: &WorktreeRow,
    radius: gpui_kit::gpui::Pixels,
    hover_bg: gpui_kit::gpui::Hsla,
    muted: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let badge = worktree
        .is_current
        .then_some(("1 active", primary, primary.opacity(0.14)));
    sidebar_meta_row(
        SharedString::from(format!("worktree-row-{}", worktree.name)),
        SharedString::from(worktree.name.clone()),
        SharedString::from(worktree.detail.clone()),
        badge,
        worktree.is_current,
        worktree.is_current,
        radius,
        hover_bg,
        muted,
        primary,
        cx,
    )
}

/// A background row: the running command and its live badge.
fn background_row(
    task: &BackgroundRow,
    radius: gpui_kit::gpui::Pixels,
    hover_bg: gpui_kit::gpui::Hsla,
    muted: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    sidebar_meta_row(
        SharedString::from(format!("background-row-{}", task.command)),
        SharedString::from(task.command.clone()),
        SharedString::from("Running"),
        Some(("Running", primary, primary.opacity(0.14))),
        true,
        false,
        radius,
        hover_bg,
        muted,
        primary,
        cx,
    )
}

/// The shared shape behind the worktree and background rows.
///
/// Sessions, worktrees, and background work all read as the prototype's
/// `.row`: a status dot, a title, a metadata line, and an optional badge. They
/// differ only in what those slots carry and whether the row is interactive,
/// so the geometry lives here once.
#[allow(clippy::too_many_arguments)]
fn sidebar_meta_row(
    id: SharedString,
    title: SharedString,
    meta: SharedString,
    badge: Option<(&'static str, gpui_kit::gpui::Hsla, gpui_kit::gpui::Hsla)>,
    active_dot: bool,
    highlighted: bool,
    radius: gpui_kit::gpui::Pixels,
    hover_bg: gpui_kit::gpui::Hsla,
    muted: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    _cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let (dot, dot_bg) = if active_dot {
        (primary, primary.opacity(0.16))
    } else {
        (muted.opacity(0.5), muted.opacity(0.12))
    };

    h_flex()
        .id(id)
        .test_support()
        .items_start()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(radius)
        .when(highlighted, move |row| row.bg(primary.opacity(0.10)))
        .hover(move |style| style.bg(hover_bg))
        .child(
            div()
                .mt_1()
                .size(rems(0.4375))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .border_1()
                .border_color(dot_bg),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .when(highlighted, move |title_div| {
                            title_div.text_color(primary)
                        })
                        .child(title),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(meta),
                        )
                        .when_some(badge, |row, (text, fg, bg)| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .rounded(radius)
                                    .bg(bg)
                                    .px_1()
                                    .text_xs()
                                    .text_color(fg)
                                    .child(SharedString::from(text)),
                            )
                        }),
                ),
        )
}

/// A session bucket for the sidebar's grouped list.
struct SessionBucket {
    label: String,
    count: usize,
    sessions: Vec<RecentSession>,
}

/// Split recent sessions into the prototype's recency buckets.
///
/// Sessions arrive newest first, so buckets are emitted in that order and a
/// short list degenerates into a single "Sessions" group rather than a lonely
/// "Today" heading.
fn session_buckets(recent: &[RecentSession], query: &str, now: i64) -> Vec<SessionBucket> {
    let matched: Vec<&RecentSession> = recent
        .iter()
        .filter(|session| {
            query.is_empty() || session.id.to_lowercase().contains(query)
        })
        .collect();

    if matched.is_empty() {
        return Vec::new();
    }

    if matched.len() < SIDEBAR_GROUP_MIN {
        return vec![SessionBucket {
            label: "Sessions".to_string(),
            count: matched.len(),
            sessions: matched.into_iter().cloned().collect(),
        }];
    }

    let mut buckets: Vec<SessionBucket> = Vec::new();
    for session in matched {
        let age = now.saturating_sub(session.updated_at_unix);
        let label = if age < 86_400 {
            "Today"
        } else if age < 7 * 86_400 {
            "This week"
        } else {
            "Earlier"
        };

        match buckets.iter_mut().find(|bucket| bucket.label == label) {
            Some(bucket) => {
                bucket.count += 1;
                bucket.sessions.push(session.clone());
            }
            None => buckets.push(SessionBucket {
                label: label.to_string(),
                count: 1,
                sessions: vec![session.clone()],
            }),
        }
    }

    buckets
}

/// Uppercase group heading with a trailing count, as in the prototype.
fn group_label(label: &str, count: usize, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .px_2()
        .pb_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(label.to_uppercase()))
        .child(div().flex_1())
        .child(SharedString::from(count.to_string()))
}

/// Human summary of live work, shared by the sidebar footer.
fn session_activity_line(state: &SessionState) -> String {
    match (state.tasks.len(), state.subagents.len()) {
        (0, 0) => "No active tasks".to_string(),
        (tasks, 0) => format!("{tasks} task(s)"),
        (0, runs) => format!("{runs} subagent run(s)"),
        (tasks, runs) => format!("{tasks} task(s) \u{b7} {runs} subagent run(s)"),
    }
}

fn sidebar_footer(
    project: &str,
    branch: Option<&str>,
    activity: &str,
    cx: &App,
) -> impl IntoElement {
    let meta = branch
        .map(|branch| format!("{branch} · {activity}"))
        .unwrap_or_else(|| activity.to_string());
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .border_t_1()
        .border_color(cx.theme().sidebar_border)
        .px_3()
        .py_2()
        .child(
            div()
                .size(rems(2.))
                .flex_shrink_0()
                .rounded(cx.theme().radius_2xl())
                .bg(cx.theme().primary)
                .text_sm()
                .text_color(cx.theme().primary_foreground)
                .flex()
                .items_center()
                .justify_center()
                .child(SharedString::from(
                    project
                        .chars()
                        .next()
                        .map(|character| character.to_uppercase().to_string())
                        .unwrap_or_else(|| "T".to_string()),
                )),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(project.to_string())),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(meta)),
                ),
        )
}

/// Sidebar as a focus-adjacent overlay below the minimum in-flow width.
fn sidebar_overlay(
    state: &SessionState,
    recent: &[RecentSession],
    current: Option<&str>,
    search: &Entity<InputState>,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    v_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(SIDEBAR_WIDTH)
        .border_r_1()
        .border_color(cx.theme().sidebar_border)
        .bg(cx.theme().sidebar)
        .shadow_xl()
        .child(sidebar(state, recent, current, search, cx))
        .id("sidebar-overlay")
        .test_support()
}

#[derive(Clone)]
struct TranscriptHeader {
    heading: SharedString,
    detail: transcript::TranscriptDetail,
}

fn transcript(
    rows: &[transcript::TranscriptRow],
    state: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    session: &SessionState,
    attachments: &[Attachment],
    header: TranscriptHeader,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let TranscriptHeader { heading, detail } = header;
    let rows = rows.to_vec();
    let empty = rows.is_empty();
    let items = rows.clone();
    // A row's click handler only receives a plain `App`, so the callback
    // travels back into this view's entity to reach `toggle_row`.
    let toggle: transcript::RowToggle = {
        let app = cx.entity().downgrade();
        Rc::new(move |index, cx: &mut App| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| app.toggle_row(index, cx));
            }
        })
    };
    let scroller = MessageScroller::new("transcript-list", state, move |index, _, cx| {
        transcript::render_row(&items[index], index, detail, &toggle, cx)
    })
    .flex_1()
    .min_h_0()
    // The prototype's `.thread` is a 720px column with an 18px gap between
    // rows, so the scroller's built-in 12px row inset and 32px row gap are
    // overridden rather than left to stack on top of each row's own spacing.
    .with_row_style(StyleRefinement::default().px(px(0.)).pb(rems(1.125)))
    .with_bottom_fade(cx.theme().background);

    let scroller = div()
        .flex_1()
        .min_h_0()
        .id("transcript-scroller")
        .test_support()
        .child(scroller);

    let mut body = v_flex()
        .w_full()
        .max_w(TRANSCRIPT_MEASURE)
        .mx_auto()
        .flex_1()
        .min_h_0()
        .child(session_intro(&heading, session, empty, detail, cx));

    body = if empty {
        body.child(empty_transcript(cx))
    } else {
        body.child(scroller)
    };

    if let Some(request) = session.request.clone() {
        body = body.child(request_panel(&request, cx));
    }

    body = body.child(prompt_composer(&composer, session, attachments, cx));

    v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .child(transcript_toolbar(session, detail, cx))
        .child(body)
        .id("transcript")
        .test_support()
}

/// The shared branch / change / run state strip above the transcript.
fn transcript_toolbar(
    session: &SessionState,
    detail: transcript::TranscriptDetail,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let mut row = h_flex()
        .w_full()
        .h(rems(2.375))
        .items_center()
        .gap(rems(0.4375))
        .border_b_1()
        .border_color(cx.theme().border)
        .px(rems(0.875));

    if let Some(branch) = session.branch.as_deref() {
        row = row.child(status_pill(IconName::GitBranch, branch, false, cx));
    }
    row = row.child(status_pill(
        IconName::File,
        format!(
            "{} file{} changed",
            session.diff.len(),
            if session.diff.len() == 1 { "" } else { "s" }
        ),
        false,
        cx,
    ));
    row = row.child(status_pill(
        if session.running {
            IconName::CircleDot
        } else {
            IconName::Circle
        },
        if session.running { "Running" } else { "Ready" },
        session.running,
        cx,
    ));

    row.child(div().flex_1()).child(
        Button::new("transcript-detail-cycle")
            .label(detail.label())
            .icon(IconName::Eye)
            .tooltip("Cycle transcript detail (Ctrl+O)")
            .accessibility_label(SharedString::from(format!(
                "Transcript detail: {}",
                detail.label()
            )))
            .ghost()
            .compact()
            .on_click(cx.listener(|this, _, _, cx| this.cycle_detail(cx))),
    )
    .id("transcript-toolbar")
    .test_support()
}

fn status_pill(
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    cx: &App,
) -> impl IntoElement {
    let (fg, bg, border) = if active {
        (
            cx.theme().primary,
            cx.theme().primary.opacity(0.12),
            cx.theme().primary.opacity(0.3),
        )
    } else {
        (
            cx.theme().muted_foreground,
            cx.theme().background,
            cx.theme().border,
        )
    };

    h_flex()
        .items_center()
        .h(rems(1.5))
        .gap_1()
        .rounded(rems(0.375))
        .border_1()
        .border_color(border)
        .bg(bg)
        .px(rems(0.4375))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(rems(0.65625))
        .text_color(fg)
        .child(icon)
        .child(label.into())
}

fn session_intro(
    heading: &SharedString,
    session: &SessionState,
    empty: bool,
    detail: transcript::TranscriptDetail,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let subtitle = if empty {
        format!(
            "Start a task in {}. The transcript stays primary; Plan, Diff, Tasks, and Files verify the work.",
            project_label(session)
        )
    } else {
        format!(
            "Continue the task in {}. Supporting detail stays in the work pane while the conversation remains readable.",
            project_label(session)
        )
    };

    // The prototype's `.head`: heading and subtitle on the left, a detail
    // cycle chip on the right, over a hairline.
    h_flex()
        .w_full()
        .items_end()
        .justify_between()
        .gap_5()
        .pt_5()
        .pb_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            v_flex()
                .min_w_0()
                .gap_2()
                .child(
                    div()
                        .text_size(rems(1.1875))
                        .font_semibold()
                        .child(heading.clone()),
                )
                .child(
                    div()
                        .max_w(rems(42.))
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(subtitle)),
                ),
        )
        .child(
            Button::new("session-intro-detail-cycle")
                .label(detail.label())
                .icon(IconName::RefreshCw)
                .tooltip("Cycle transcript detail (Ctrl+O)")
                .accessibility_label(SharedString::from(format!(
                    "Transcript detail: {}",
                    detail.label()
                )))
                .outline()
                .compact()
                .on_click(cx.listener(|this, _, _, cx| this.cycle_detail(cx))),
        )
}

/// First-run placeholder shown before the transcript has any rows.
fn empty_transcript(cx: &mut Context<TactApp>) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_h_0()
        .items_start()
        .gap_3()
        .px_6()
        .pt_6()
        .pb_8()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .text_color(cx.theme().primary)
                .child(IconName::Sparkles)
                .child(SharedString::from("No messages yet")),
        )
        .child(
            div()
                .max_w(rems(34.))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(
                    "Write the first message below. Tool activity, plans, diffs, tasks, and files appear as the agent works.",
                )),
        )
        .child(
            Button::new("transcript-empty-focus")
                .label("Write a message")
                .ghost()
                .compact()
                .on_click(cx.listener(|this, _, window, cx| this.focus_composer(window, cx))),
        )
        .id("transcript-empty")
        .test_support()
}

/// A blocking choice from the agent: permission prompts and `ask_user`.
fn request_panel(request: &Request, cx: &mut Context<TactApp>) -> impl IntoElement {
    let mut panel = v_flex()
        .w_full()
        .gap_2()
        .mx_6()
        .mb_2()
        .rounded(cx.theme().radius_2xl())
        .border_1()
        .border_color(cx.theme().primary)
        .bg(cx.theme().popover)
        .p_3()
        .child(
            div()
                .text_sm()
                .child(SharedString::from(request.prompt.clone())),
        )
        .id("request-panel")
        .test_support();

    for (index, option) in request.options.iter().enumerate() {
        let selected = request.selected.contains(&index);
        panel = panel.child(
            Button::new(SharedString::from(format!("request-option-{index}")))
                .label(option.clone())
                .ghost()
                .compact()
                .toggled(selected)
                .on_click(cx.listener(move |this, _, _, cx| this.choose(index, cx))),
        );
    }

    let mut actions = h_flex().w_full().gap_1().child(div().flex_1());
    if request.multi {
        actions = actions.child(
            Button::new("request-confirm")
                .label("Confirm")
                .primary()
                .compact()
                .on_click(cx.listener(|this, _, _, cx| this.confirm_multi(cx))),
        );
    }
    panel.child(
        actions.child(
            Button::new("request-cancel")
                .label("Cancel")
                .ghost()
                .compact()
                .on_click(cx.listener(|this, _, _, cx| this.cancel_request(cx))),
        ),
    )
}

fn prompt_composer(
    composer: &Entity<TextareaState>,
    session: &SessionState,
    attachments: &[Attachment],
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let running = session.running;
    let (primary_icon, primary_label, primary_action) = if running {
        (IconName::Pause, "Stop", "Stop the active turn")
    } else {
        (IconName::ArrowUp, "Send", "Send message")
    };
    let usage = session
        .usage
        .as_ref()
        .map(|usage| format_usage(usage.prompt, usage.total));
    let draft = composer.read(cx).value().to_string();
    let suggestions = composer::suggestions(&draft, session.workdir.as_deref());
    let owner = cx.weak_entity();
    let add_owner = owner.clone();
    let model_owner = owner.clone();
    let permission_owner = owner.clone();
    let model_options = [
        "claude-sonnet-4-5",
        "claude-opus-4-1",
        "gpt-5",
        "deepseek-chat",
        "kimi-k2-0905-preview",
    ];
    let current_model = session
        .model
        .as_ref()
        .map(|model| model.model.clone())
        .unwrap_or_else(|| "Tact".to_string());
    let current_effort = session
        .model
        .as_ref()
        .and_then(|model| model.reasoning_effort.clone());
    let current_budget = session
        .model
        .as_ref()
        .and_then(|model| model.thinking_budget);
    let permission_modes = [("auto", "Auto"), ("default", "Ask"), ("plan", "Plan")];
    let current_permission = session.permission_mode.clone();
    let muted_foreground = cx.theme().muted_foreground;
    let permission_label = permission_modes
        .iter()
        .find(|(mode, _)| *mode == current_permission.as_str())
        .map(|(_, label)| *label)
        .unwrap_or("Auto");

    let add_popover = Popover::new("composer-add-popover")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(
            Button::new("composer-add")
                .icon(IconName::Plus)
                .tooltip("Add attachment, skill, connector, or plugin")
                .accessibility_label("Add context")
                .ghost()
                .compact(),
        )
        .content(move |_state, _window, _cx| {
            let file_owner = add_owner.clone();
            let skill_owner = add_owner.clone();
            let connector_owner = add_owner.clone();
            let plugin_owner = add_owner.clone();
            v_flex()
                .id("composer-add-panel")
                .test_support()
                .gap_1()
                .min_w(rems(13.))
                .child(
                    Button::new("composer-add-file")
                        .icon(IconName::File)
                        .label("Attach file")
                        .ghost()
                        .compact()
                        .on_click(move |_, window, cx| {
                            let _ = file_owner
                                .update(cx, |app, cx| app.attach_files(window, cx));
                        }),
                )
                .child(
                    Button::new("composer-add-skill")
                        .icon(IconName::Asterisk)
                        .label("Skills")
                        .ghost()
                        .compact()
                        .on_click(move |_, window, cx| {
                            let _ = skill_owner
                                .update(cx, |app, cx| app.open_skill_completion(window, cx));
                        }),
                )
                .child(
                    Button::new("composer-add-connector")
                        .icon(IconName::Network)
                        .label("Connectors")
                        .ghost()
                        .compact()
                        .on_click(move |_, _, cx| {
                            let _ = connector_owner.update(cx, |app, cx| {
                                app.send_command(tact_protocol::UserCommand::McpList, cx)
                            });
                        }),
                )
                .child(
                    Button::new("composer-add-plugin")
                        .icon(IconName::Settings)
                        .label("Plugins")
                        .ghost()
                        .compact()
                        .on_click(move |_, _, cx| {
                            let _ = plugin_owner.update(cx, |app, cx| {
                                app.push_system_row(
                                    "Plugin discovery is not configured in v1; use installed skills or MCP connectors."
                                        .to_string(),
                                    cx,
                                )
                            });
                        }),
                )
        });

    let model_popover = Popover::new("composer-model-popover")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(
            Button::new("composer-model")
                .label(current_model.clone())
                .icon(IconName::ChevronDown)
                .tooltip("Model and reasoning controls")
                .accessibility_label("Model and reasoning controls")
                .ghost()
                .compact(),
        )
        .content(move |_state, _window, _cx| {
            let mut panel = v_flex()
                .id("composer-model-panel")
                .test_support()
                .gap_1()
                .min_w(rems(18.))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(SharedString::from("Model")),
                );
            for model in model_options.iter() {
                let owner = model_owner.clone();
                let model = model.to_string();
                let selected = model == current_model;
                panel = panel.child(
                    Button::new(SharedString::from(format!(
                        "composer-model-{}",
                        model.replace(['/', ':', ' '], "-")
                    )))
                    .label(model.clone())
                    .ghost()
                    .compact()
                    .toggled(selected)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |app, cx| app.set_model(model.clone(), cx));
                    }),
                );
            }
            panel = panel.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(SharedString::from("Reasoning effort")),
            );
            for effort in [
                None,
                Some("low"),
                Some("medium"),
                Some("high"),
                Some("xhigh"),
                Some("max"),
            ] {
                let owner = model_owner.clone();
                let value = effort.map(str::to_string);
                let label = value.clone().unwrap_or_else(|| "Auto".to_string());
                let id = format!("composer-effort-{}", label.to_ascii_lowercase());
                panel = panel.child(
                    Button::new(id)
                        .label(label)
                        .ghost()
                        .compact()
                        .toggled(value == current_effort)
                        .on_click(move |_, _, cx| {
                            let _ = owner
                                .update(cx, |app, cx| app.set_reasoning_effort(value.clone(), cx));
                        }),
                );
            }
            panel = panel.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(SharedString::from("Thinking budget")),
            );
            for budget in [0_u32, 8_192, 16_384, 32_768, 65_536] {
                let owner = model_owner.clone();
                let selected = current_budget == Some(budget);
                let label = if budget == 0 {
                    "Off".to_string()
                } else {
                    format!("{}k", budget / 1_024)
                };
                panel = panel.child(
                    Button::new(SharedString::from(format!("composer-budget-{budget}")))
                        .label(label)
                        .ghost()
                        .compact()
                        .toggled(selected)
                        .on_click(move |_, _, cx| {
                            let _ = owner
                                .update(cx, |app, cx| app.set_thinking_budget(budget as usize, cx));
                        }),
                );
            }
            panel
        });

    let permission_popover = Popover::new("composer-permission-popover")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(
            Button::new("composer-permission")
                .label(permission_label)
                .icon(IconName::ChevronDown)
                .tooltip("Permission mode")
                .accessibility_label("Permission mode")
                .ghost()
                .compact(),
        )
        .content(move |_state, _window, _cx| {
            let mut panel = v_flex()
                .id("composer-permission-panel")
                .test_support()
                .gap_1();
            for (mode, label) in permission_modes.iter().copied() {
                let owner = permission_owner.clone();
                let value = mode.to_string();
                panel = panel.child(
                    Button::new(SharedString::from(format!("composer-permission-{mode}")))
                        .label(label)
                        .ghost()
                        .compact()
                        .toggled(value == current_permission)
                        .on_click(move |_, _, cx| {
                            let _ = owner
                                .update(cx, |app, cx| app.set_permission_mode(value.clone(), cx));
                        }),
                );
            }
            panel
        });

    let usage_snapshot = session.usage.clone();
    let usage_for_panel = usage_snapshot.clone();
    let usage_ring = usage_snapshot.as_ref().map(|usage| {
        let percentage = if usage.total == 0 {
            0.0
        } else {
            usage.prompt as f32 / usage.total as f32 * 100.0
        };
        let label = format_usage(usage.prompt, usage.total);
        Popover::new("composer-usage-popover")
            .anchor(gpui_kit::Anchor::TopLeft)
            .trigger(
                Button::new("composer-usage")
                    .tooltip("Context usage")
                    .accessibility_label("Context usage")
                    .ghost()
                    .compact()
                    .child(
                        ProgressCircle::new("composer-usage-ring")
                            .value(percentage)
                            .small()
                            .accessibility_label(label.clone()),
                    ),
            )
            .content(move |_state, _window, _cx| usage_panel(usage_for_panel.clone()))
    });

    let mut controls = h_flex()
        .w_full()
        .items_center()
        .gap_1()
        .child(add_popover)
        .child(model_popover)
        .child(permission_popover);

    if let Some(usage_ring) = usage_ring {
        controls = controls.child(usage_ring);
    }

    controls = controls.child(div().flex_1());

    if let Some(usage) = usage {
        controls = controls.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(usage)),
        );
    }

    controls = controls.child(
        Button::new("composer-primary")
            .icon(primary_icon)
            .label(primary_label)
            .tooltip(primary_action)
            .accessibility_label(primary_action)
            .primary()
            .compact()
            .on_click(cx.listener(|this, _, window, cx| {
                if this.state.running {
                    this.stop(cx);
                } else {
                    this.submit(window, cx);
                }
            })),
    );

    let mut body = v_flex()
        .w_full()
        .gap_2()
        .rounded(cx.theme().radius_2xl())
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .p_2();

    if !attachments.is_empty() {
        let mut chips = h_flex()
            .w_full()
            .flex_wrap()
            .gap_1()
            .id("composer-attachments")
            .test_support();
        for (index, attachment) in attachments.iter().enumerate() {
            let remove = Button::new(SharedString::from(format!(
                "composer-attachment-remove-{index}"
            )))
            .icon(IconName::Close)
            .tooltip("Remove attachment (Ctrl+Shift+Backspace removes the last chip)")
            .accessibility_label("Remove attachment")
            .ghost()
            .compact();
            let remove = remove.on_click(cx.listener(move |this, _, _, cx| {
                this.remove_attachment(index, cx);
            }));
            chips = chips.child(
                h_flex()
                    .id(SharedString::from(format!("composer-attachment-{index}")))
                    .test_support()
                    .items_center()
                    .gap_1()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .px_2()
                    .py_0p5()
                    .child(
                        div()
                            .max_w(rems(18.))
                            .truncate()
                            .text_xs()
                            .child(SharedString::from(attachment.label.clone())),
                    )
                    .child(remove),
            );
        }
        body = body.child(chips);
    }

    if !suggestions.is_empty() {
        let mut list = v_flex()
            .w_full()
            .gap_0p5()
            .id("composer-suggestions")
            .test_support();
        for (index, suggestion) in suggestions.iter().enumerate() {
            let label = match suggestion.kind {
                composer::SuggestionKind::File => format!("@{}", suggestion.label),
                composer::SuggestionKind::Skill => suggestion.label.clone(),
            };
            let icon = match suggestion.kind {
                composer::SuggestionKind::File => IconName::File,
                composer::SuggestionKind::Skill => IconName::Asterisk,
            };
            list = list.child(
                Button::new(SharedString::from(format!("composer-suggestion-{index}")))
                    .icon(icon)
                    .label(label)
                    .ghost()
                    .compact()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.accept_suggestion(index, window, cx);
                    })),
            );
        }
        body = body.child(list);
    }

    body = body
        .child(
            div().id("prompt-composer-input").test_support().child(
                Textarea::new(composer)
                    .appearance(false)
                    .bordered(false)
                    .w_full()
                    .min_h(rems(2.5))
                    .accessibility_id("prompt-composer-input")
                    .aria_label("Message Tact"),
            ),
        )
        .child(controls);

    v_flex()
        .w_full()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .px_6()
        .pt_3()
        .pb_4()
        .child(body)
        .id("composer")
        .test_support()
}

fn usage_panel(usage: Option<tact_protocol::TokenUsageInfo>) -> impl IntoElement {
    let usage = usage.unwrap_or_default();
    v_flex()
        .id("composer-usage-panel")
        .test_support()
        .gap_1()
        .min_w(rems(16.))
        .child(SharedString::from(format!(
            "Prompt: {} tokens",
            usage.prompt
        )))
        .child(SharedString::from(format!(
            "Completion: {} tokens",
            usage.completion
        )))
        .child(SharedString::from(format!("Total: {} tokens", usage.total)))
        .child(SharedString::from(format!(
            "Cache: {} hit / {} miss",
            usage.prompt_cache_hit_tokens, usage.prompt_cache_miss_tokens
        )))
        .child(SharedString::from(format!(
            "Reasoning: {} tokens",
            usage.reasoning_tokens
        )))
}

/// Compact usage label: context consumed against the completion budget.
fn format_usage(prompt: u32, total: u32) -> String {
    if total == 0 {
        return String::new();
    }
    format!("{prompt} / {total} tok")
}

/// Resolve the workspace branch once when a session is adopted. The status bar
/// should not shell out on every frame.
fn git_branch(workdir: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    (!branch.is_empty() && branch != "HEAD").then(|| branch.to_string())
}

fn work_pane(
    selected: WorkPane,
    state: &SessionState,
    files: &FilesPane,
    diffs: &mut pane::DiffPane,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    v_flex()
        .flex_shrink_0()
        .w(WORK_PANE_WIDTH)
        .h_full()
        .border_l_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .child(pane::view(selected, state, files, diffs, cx))
        .id("work-pane")
        .test_support()
}

/// Work pane as a right drawer when the window is too narrow for three columns.
fn work_pane_drawer(
    selected: WorkPane,
    state: &SessionState,
    files: &FilesPane,
    diffs: &mut pane::DiffPane,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    v_flex()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .w(WORK_PANE_WIDTH)
        .border_l_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .shadow_xl()
        .child(pane::view(selected, state, files, diffs, cx))
        .id("work-pane")
        .test_support()
}

fn status_bar(workspace: Workspace, state: &SessionState, cx: &App) -> impl IntoElement {
    let project = state
        .workdir
        .as_deref()
        .and_then(std::path::Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(workspace.label())
        .to_string();

    // Diff totals across recorded changes, shown as the prototype's green/red pair.
    let added: u32 = state.diff.iter().filter_map(|entry| entry.added).sum();
    let removed: u32 = state.diff.iter().filter_map(|entry| entry.removed).sum();

    let permission = match state.permission_mode.as_str() {
        "plan" => "Plan mode",
        "default" | "ask" => "Ask permission",
        _ => "Auto approve",
    };

    let running = state
        .subagents
        .iter()
        .filter(|run| run.status == tact_protocol::SubagentStatusSnapshot::Running)
        .count();

    let context = state.usage.as_ref().map(|usage| {
        let denominator = usage.prompt.saturating_add(usage.completion).max(1);
        format!("{}% context", (usage.prompt * 100 / denominator).min(100))
    });

    let mut bar = h_flex()
        .w_full()
        .h(STATUS_BAR_HEIGHT)
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().tab_bar)
        .px_3()
        .text_size(rems(0.65625))
        .text_color(cx.theme().muted_foreground)
        .child(status_item(
            Some(IconName::Box),
            project,
            cx.theme().foreground,
            cx,
        ));

    if let Some(branch) = state.branch.as_deref() {
        bar = bar.child(status_item(
            Some(IconName::GitBranch),
            branch.to_string(),
            cx.theme().muted_foreground,
            cx,
        ));
    }

    bar = bar.child(
        h_flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .text_color(cx.theme().muted_foreground)
            .child(div().text_color(cx.theme().accent_foreground).child("\u{25cf}"))
            .child(SharedString::from(permission)),
    );

    if added > 0 || removed > 0 {
        bar = bar.child(
            h_flex()
                .items_center()
                .gap_1()
                .font_family(cx.theme().mono_font_family.clone())
                .child(
                    div()
                        .text_color(cx.theme().success)
                        .child(SharedString::from(format!("+{added}"))),
                )
                .child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(format!("\u{2212}{removed}"))),
                ),
        );
    }

    if let Some(turns) = state.turns {
        let label = match turns {
            (taken, Some(max)) => format!("turn {taken}/{max}"),
            (taken, None) => format!("turn {taken}"),
        };
        bar = bar.child(status_item(None, label, cx.theme().muted_foreground, cx));
    }

    bar = bar.child(div().flex_1());

    if let Some(context) = context {
        bar = bar.child(status_item(None, context, cx.theme().muted_foreground, cx));
    }

    // The prototype closes the bar with the account balance; the provider only
    // reports one for accounts that track it, so the chip is conditional.
    if let Some(balance) = balance_label(state) {
        bar = bar.child(status_item(None, balance, cx.theme().muted_foreground, cx));
    }

    if running > 0 {
        bar = bar.child(status_item(
            None,
            format!("{running} running"),
            cx.theme().primary,
            cx,
        ));
    }

    bar.id("status-bar").test_support()
}

/// The account balance chip, when the provider reports one.
///
/// Providers that surface a balance send per-currency entries; the bar shows
/// the first one, which is the currency the account actually bills in.
fn balance_label(state: &SessionState) -> Option<String> {
    let tact_protocol::AccountUpdate::Balance(info) = state.account.as_ref()? else {
        return None;
    };
    let entry = info.balance_infos.first()?;
    let symbol = match entry.currency.as_str() {
        "USD" => "$",
        "CNY" => "\u{a5}",
        other => return Some(format!("{:.2} {other}", entry.total_balance)),
    };
    Some(format!("{symbol}{:.2}", entry.total_balance))
}

/// One status-bar segment: an optional glyph plus text.
fn status_item(
    icon: Option<IconName>,
    label: String,
    color: gpui_kit::Hsla,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .text_color(color)
        .font_family(cx.theme().font_family.clone())
        .children(icon)
        .child(SharedString::from(label))
}

fn settings_panel(
    owner: gpui_kit::WeakEntity<TactApp>,
    _window: &mut Window,
    _cx: &mut App,
) -> impl IntoElement {
    let theme_row = SettingItem::render(move |_, _window, cx| {
        let dark = Theme::global(cx).mode.is_dark();
        setting_copy(
            "Theme",
            "Switch the shell between Tact's Anthropic light and dark palettes.",
            cx,
        )
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("settings-theme-light")
                        .label("Light")
                        .toggled(!dark)
                        .ghost()
                        .compact()
                        .on_click(|_, window, cx| {
                            if let Err(err) = theme::activate(ThemeMode::Light, Some(window), cx) {
                                tracing::warn!("Cannot switch the Tact theme: {err:#}");
                            }
                        }),
                )
                .child(
                    Button::new("settings-theme-dark")
                        .label("Dark")
                        .toggled(dark)
                        .ghost()
                        .compact()
                        .on_click(|_, window, cx| {
                            if let Err(err) = theme::activate(ThemeMode::Dark, Some(window), cx) {
                                tracing::warn!("Cannot switch the Tact theme: {err:#}");
                            }
                        }),
                ),
        )
    });

    let thinking_owner = owner.clone();
    let thinking_row = SettingItem::render(move |_, _window, cx| {
        let checked = thinking_owner
            .upgrade()
            .map(|app| app.read(cx).detail.shows_thinking())
            .unwrap_or(false);
        let owner = thinking_owner.clone();
        setting_copy(
            "Show reasoning",
            "Keep thinking blocks visible in the transcript; turn this off for a quieter answer.",
            cx,
        )
        .child(
            Switch::new("settings-show-thinking")
                .checked(checked)
                .accessibility_label("Show reasoning in transcript")
                .on_click(move |checked, _, cx| {
                    let _ = owner.update(cx, |app, cx| {
                        if *checked {
                            if app.detail == transcript::TranscriptDetail::Normal {
                                app.detail = transcript::TranscriptDetail::Thinking;
                            }
                        } else {
                            app.detail = transcript::TranscriptDetail::Normal;
                        }
                        app.transcript_state
                            .update(cx, |state, cx| state.remeasure(cx));
                        cx.notify();
                    });
                }),
        )
    });

    let follow_owner = owner.clone();
    let follow_row = SettingItem::render(move |_, _window, cx| {
        let checked = follow_owner
            .upgrade()
            .map(|app| app.read(cx).follow_tail)
            .unwrap_or(true);
        let owner = follow_owner.clone();
        setting_copy(
            "Follow streaming output",
            "Keep the newest assistant output in view while a turn is running.",
            cx,
        )
        .child(
            Switch::new("settings-follow-tail")
                .checked(checked)
                .accessibility_label("Follow streaming output")
                .on_click(move |checked, _, cx| {
                    let _ = owner.update(cx, |app, cx| {
                        app.follow_tail = *checked;
                        if app.follow_tail {
                            app.transcript_state
                                .update(cx, |state, cx| state.scroll_to_end(cx));
                        }
                        cx.notify();
                    });
                }),
        )
    });

    let info_owner = owner;
    let session_info = SettingItem::render(move |_, _window, cx| {
        let (model, workdir, usage, turns) = info_owner
            .upgrade()
            .map(|app| {
                let app = app.read(cx);
                let model = app
                    .state
                    .model
                    .as_ref()
                    .map(|model| model.model.clone())
                    .unwrap_or_else(|| "Provider default".to_string());
                let workdir = app
                    .state
                    .workdir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Not connected".to_string());
                let usage = app
                    .state
                    .usage
                    .as_ref()
                    .map(|usage| format_usage(usage.prompt, usage.total))
                    .filter(|usage| !usage.is_empty())
                    .unwrap_or_else(|| "No usage reported yet".to_string());
                let turns = app
                    .state
                    .turns
                    .map(|(taken, max)| match max {
                        Some(max) => format!("{taken}/{max}"),
                        None => taken.to_string(),
                    })
                    .unwrap_or_else(|| "Idle".to_string());
                (model, workdir, usage, turns)
            })
            .unwrap_or_else(|| {
                (
                    "Session unavailable".to_string(),
                    "Not connected".to_string(),
                    "No usage reported yet".to_string(),
                    "Idle".to_string(),
                )
            });

        v_flex()
            .w_full()
            .gap_2()
            .child(settings_value("Model", &model, cx))
            .child(settings_value("Workspace", &workdir, cx))
            .child(settings_value("Turns", &turns, cx))
            .child(settings_value("Token usage", &usage, cx))
    });

    div().w_full().h(px(540.)).child(
        Settings::new("tact-settings").pages([
            SettingPage::new("Appearance")
                .default_open(true)
                .description("Theme and transcript reading controls.")
                .group(
                    SettingGroup::new()
                        .title("Theme")
                        .description("The shell stays Anthropic-first in both modes.")
                        .item(theme_row),
                )
                .group(
                    SettingGroup::new()
                        .title("Reading")
                        .description("Controls apply to the current window immediately.")
                        .item(thinking_row)
                        .item(follow_row),
                ),
            SettingPage::new("Session")
                .default_open(true)
                .description("Live values from the attached agent session.")
                .group(
                    SettingGroup::new()
                        .title("Current session")
                        .description("Model and usage values update as the session runs.")
                        .item(session_info),
                ),
        ]),
    )
}

fn setting_copy(title: &str, description: &str, cx: &App) -> gpui_kit::Div {
    v_flex()
        .min_w_0()
        .gap_1()
        .child(div().text_sm().child(SharedString::from(title.to_string())))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(description.to_string())),
        )
}

fn settings_value(label: &str, value: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .min_w_0()
        .gap_3()
        .child(
            div()
                .w_24()
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_sm()
                .child(SharedString::from(value.to_string())),
        )
}

/// Which session `Ctrl+Tab` moves to, given the list length and the current
/// row. `None` means "nothing to switch to": an empty list, or the only session
/// already open.
fn next_session_index(len: usize, current: Option<usize>) -> Option<usize> {
    if len == 0 || (len == 1 && current.is_some()) {
        return None;
    }
    Some(match current {
        Some(index) => (index + 1) % len,
        None => 0,
    })
}

/// Which session `Ctrl+Shift+Tab` moves to. This mirrors [`next_session_index`]
/// in the opposite direction while preserving the "nothing else to switch to"
/// result for an empty list or a lone open session.
fn previous_session_index(len: usize, current: Option<usize>) -> Option<usize> {
    if len == 0 || (len == 1 && current.is_some()) {
        return None;
    }
    Some(match current {
        Some(0) => len - 1,
        Some(index) => index - 1,
        None => len - 1,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        SessionBucket, background_rows, balance_label, next_session_index,
        previous_session_index, session_buckets, worktree_rows,
    };
    use crate::{RecentSession, session::SessionState};

    /// The worktree group reads the repository the session is rooted in.
    #[test]
    fn worktrees_come_from_the_session_workspace() {
        let mut state = SessionState::default();
        assert!(
            worktree_rows(&state).is_empty(),
            "a detached session has no worktrees to show"
        );

        state.workdir = Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let rows = worktree_rows(&state);
        assert!(
            !rows.is_empty(),
            "the crate lives in a git worktree, so the list must not be empty"
        );
        assert!(
            rows.iter().any(|row| row.is_current),
            "the session's own worktree is marked current"
        );
        assert!(
            rows.iter().all(|row| !row.name.is_empty() && !row.detail.is_empty()),
            "every row carries a branch and a directory"
        );
    }

    /// Background rows mirror the recorded running commands.
    #[test]
    fn background_rows_mirror_recorded_commands() {
        let mut state = SessionState::default();
        assert!(background_rows(&state).is_empty());

        state.background = vec!["cargo test -p tact".to_string()];
        let rows = background_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "cargo test -p tact");
    }

    /// The balance chip follows the provider's reported currency.
    #[test]
    fn balance_uses_the_reported_currency() {
        let state = SessionState::default();
        assert!(balance_label(&state).is_none(), "no account update, no chip");

        let with_balance = |currency: &str, total: f64| SessionState {
            account: Some(tact_protocol::AccountUpdate::Balance(
                tact_protocol::BalanceInfo {
                    is_available: true,
                    balance_infos: vec![tact_protocol::BalanceEntry {
                        currency: currency.to_string(),
                        total_balance: total,
                        granted_balance: 0.0,
                        topped_up_balance: total,
                    }],
                },
            )),
            ..SessionState::default()
        };

        assert_eq!(
            balance_label(&with_balance("USD", 18.42)).as_deref(),
            Some("$18.42")
        );
        assert_eq!(
            balance_label(&with_balance("CNY", 100.0)).as_deref(),
            Some("\u{a5}100.00")
        );
        assert_eq!(
            balance_label(&with_balance("EUR", 5.0)).as_deref(),
            Some("5.00 EUR"),
            "an unknown currency keeps its code rather than guessing a symbol"
        );
    }

    fn session(id: &str, age_seconds: i64, now: i64) -> RecentSession {
        RecentSession {
            id: id.to_string(),
            updated_at_unix: now - age_seconds,
            message_count: 1,
        }
    }

    fn labels(buckets: &[SessionBucket]) -> Vec<&str> {
        buckets.iter().map(|bucket| bucket.label.as_str()).collect()
    }

    #[test]
    fn a_short_list_stays_in_one_ungrouped_section() {
        let now = 1_700_000_000;
        let recent = vec![
            session("aaaaaaaa-1", 30, now),
            session("bbbbbbbb-2", 90, now),
        ];

        let buckets = session_buckets(&recent, "", now);

        assert_eq!(labels(&buckets), vec!["Sessions"]);
        assert_eq!(buckets[0].count, 2);
    }

    #[test]
    fn a_long_list_splits_into_recency_buckets_newest_first() {
        let now = 1_700_000_000;
        let day = 86_400;
        let recent = vec![
            session("aaaaaaaa-1", 60, now),
            session("bbbbbbbb-2", 2 * day, now),
            session("cccccccc-3", 3 * day, now),
            session("dddddddd-4", 10 * day, now),
        ];

        let buckets = session_buckets(&recent, "", now);

        assert_eq!(labels(&buckets), vec!["Today", "This week", "Earlier"]);
        assert_eq!(buckets[0].count, 1);
        assert_eq!(buckets[1].count, 2);
        assert_eq!(buckets[2].count, 1);
    }

    #[test]
    fn the_filter_matches_session_ids_case_insensitively() {
        let now = 1_700_000_000;
        let recent = vec![
            session("AAAAAAAA-1", 60, now),
            session("bbbbbbbb-2", 60, now),
        ];

        let buckets = session_buckets(&recent, "aaaa", now);

        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].sessions.len(), 1);
        assert_eq!(buckets[0].sessions[0].id, "AAAAAAAA-1");
    }

    #[test]
    fn a_filter_with_no_matches_renders_no_groups() {
        let now = 1_700_000_000;

        assert!(session_buckets(&[session("aaaaaaaa-1", 60, now)], "zzz", now).is_empty());
    }

    #[test]
    fn cycling_wraps_and_skips_a_lone_open_session() {
        assert_eq!(next_session_index(0, None), None, "nothing to switch to");
        assert_eq!(
            next_session_index(1, Some(0)),
            None,
            "the only session is already open"
        );
        assert_eq!(
            next_session_index(1, None),
            Some(0),
            "a lone stored session is still resumable"
        );
        assert_eq!(next_session_index(3, None), Some(0));
        assert_eq!(next_session_index(3, Some(0)), Some(1));
        assert_eq!(next_session_index(3, Some(2)), Some(0), "wraps at the end");
    }

    #[test]
    fn reverse_cycling_wraps_to_the_end() {
        assert_eq!(previous_session_index(0, None), None);
        assert_eq!(previous_session_index(1, Some(0)), None);
        assert_eq!(previous_session_index(1, None), Some(0));
        assert_eq!(previous_session_index(3, None), Some(2));
        assert_eq!(previous_session_index(3, Some(0)), Some(2));
        assert_eq!(previous_session_index(3, Some(2)), Some(1));
    }
}
