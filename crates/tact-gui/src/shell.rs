//! The Tact window shell.
//!
//! Five regions, laid out as one continuous workspace: a fixed title bar, a
//! session sidebar, the transcript, a collapsible work pane, and a fixed status
//! bar. The transcript owns a virtualized message scroller and a bottom-docked
//! composer, and is driven by the live agent session when one is attached.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::base::motion::{Presence, Transition};
use gpui_kit::base::{Selectable, StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Root, Sizable as _, Theme, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    command::{Command, CommandState},
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    message_scroller::{MessageScroller, MessageScrollerState},
    notification::Notification,
    popover::Popover,
    progress::ProgressCircle,
    scroll::ScrollableElement as _,
    setting::{SettingGroup, SettingItem, SettingPage, Settings},
    switch::Switch,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Div, Entity, FocusHandle,
    Focusable as _, InteractiveElement, IntoElement, MouseButton, ParentElement as _, Pixels, Rems,
    Render, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled as _, Subscription, Window, div, px, relative, rems,
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
use crate::theme::accent_tint;
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

/// The prototype's three column widths, which shrink together at 1320 px:
/// `@media(max-width:1320px){:root{--sidebar:244px;--work:374px}
/// .thread{width:min(680px,calc(100% - 36px))}}`.
///
/// Only the columns shrink. The work pane keeps 420 px in its drawer form,
/// because the prototype's own narrow rule resets it
/// (`@media(max-width:1120px)` gives `.work{width:min(420px,88vw)}`), and the
/// floating sidebar keeps the column's width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Columns {
    sidebar: Rems,
    work_pane: Rems,
    thread: Rems,
    /// One side's share of the thread's `100% - 48px` gutter.
    gutter: Rems,
}

impl Columns {
    /// 1320 px at the default rem, the width the prototype's first media query
    /// starts at.
    const NARROW_UNDER: Rems = rems(82.5);

    fn for_width(width: Pixels, rem_size: Pixels) -> Self {
        if width <= Self::NARROW_UNDER.to_pixels(rem_size) {
            Self {
                sidebar: rems(15.25),
                work_pane: rems(23.375),
                thread: rems(42.5),
                gutter: rems(1.125),
            }
        } else {
            Self {
                sidebar: SIDEBAR_WIDTH,
                work_pane: WORK_PANE_WIDTH,
                thread: TRANSCRIPT_MEASURE,
                gutter: rems(1.5),
            }
        }
    }
}
/// Width from which the work pane sits in the layout (1280 px at the default
/// rem). Below it the drawer form applies (Phase 6).
const WORK_PANE_IN_FLOW_FROM: Rems = rems(80.);

/// The prototype's only motion token, `--ease:cubic-bezier(.23,1,.32,1)`, and
/// the `180ms` it gives `.work` and `.sidebar`.
const OVERLAY_SLIDE: Duration = Duration::from_millis(180);
/// `transform:translateX(105%)` of the drawer's own width, which is how far
/// `@media(max-width:1120px)` parks `.work` past the right edge.
const WORK_PANE_DRAWER_OFFSET: Rems = rems(WORK_PANE_WIDTH.0 * 1.05);
/// The same `105%` for the floating sidebar, which the prototype parks past
/// the left edge instead (`translateX(-105%)`).
const SIDEBAR_OVERLAY_OFFSET: Rems = rems(SIDEBAR_WIDTH.0 * 1.05);
/// The presence keys the two sliding overlays keep their transition under.
const WORK_PANE_DRAWER_PRESENCE: &str = "work-pane-drawer-slide";
const SIDEBAR_OVERLAY_PRESENCE: &str = "sidebar-overlay-slide";

/// `transition:transform 180ms var(--ease)` as `gpui-base`'s motion layer
/// wants it. Reused so both sliding surfaces share one timing.
fn overlay_slide() -> Transition {
    Transition::new(OVERLAY_SLIDE).ease(cubic_bezier(0.23, 1.0, 0.32, 1.0))
}

/// The inset that stands in for `translateX(±105%)`.
///
/// This GPUI has no paint-level transform, so the offset rides on the inset
/// the overlay is anchored by: at `progress == 1` the overlay rests, and at
/// `0` it sits a full width plus the prototype's 5% offscreen.
fn overlay_inset(offset: Rems, progress: f32) -> Rems {
    rems(offset.0 * (progress - 1.0))
}

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

    /// The work pane this preset opens.
    ///
    /// The prototype's `.tab` handler pairs each top-level preset with the
    /// pane that belongs to it: chat shows the plan, agent the tasks, code the
    /// diff.
    pub fn work_pane(self) -> WorkPane {
        match self {
            Self::Chat => WorkPane::Plan,
            Self::Agent => WorkPane::Tasks,
            Self::Code => WorkPane::Diff,
        }
    }
}

/// One command the shell can run, reachable from both the keyboard contract
/// and the matching palette row.
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

/// The application shell view.
pub struct TactApp {
    workspace: Workspace,
    sidebar_open: bool,
    work_pane_open: bool,
    work_pane: WorkPane,
    files: FilesPane,
    /// Whether the Files pane drew on the previous frame, so its cached
    /// workspace walk can be dropped when the pane comes back into view.
    files_listed: bool,
    /// Lazily loaded `git diff` bodies for the Diff pane.
    diffs: pane::DiffPane,
    /// Transcript rows and the live-row bookkeeping behind them.
    conversation: Conversation,
    /// Session state the panes, composer, and status bar read.
    state: SessionState,
    /// Prompts typed while a turn was in flight, oldest first.
    queued: VecDeque<String>,
    /// Toasts raised by handlers that run without a window.
    ///
    /// Notifications are pushed through the window's `Root`, so updates that
    /// arrive on the session pump queue their message here and the next render
    /// delivers it.
    toasts: Vec<Notification>,
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
    /// The row the sidebar marks as open when no session is attached.
    ///
    /// The offline shell has no session to highlight, so the row a click
    /// picked plays that part; the design preview starts on the newest one.
    preview_current: Option<String>,
    /// Whether this shell runs without an agent runtime.
    ///
    /// [`Self::new`], [`Self::with_sessions`], and [`Self::preview`] are
    /// offline: they render the whole surface but own no session, so their
    /// session controls must not start one. [`Self::connect`] leaves this
    /// false even when startup failed, so a click can retry.
    offline: bool,
    /// Keeps the event pump alive for as long as the window is open.
    _pump: Option<session::Pump>,
    /// Command palette interaction state, retained while its dialog is open,
    /// plus whether that dialog is still on the window's stack. A palette row
    /// dispatches its action while the palette is open, and the palette's own
    /// confirmation arrives one frame later; the flag lets that late callback
    /// tell "close the palette" apart from "a row already replaced it" — the
    /// settings row pops the palette itself so its dialog survives.
    palette: Option<Entity<CommandState>>,
    palette_live: bool,
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
        let mut app = Self::build(window, cx, None, Vec::new());
        app.offline = true;
        app
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
        let mut app = Self::build(window, cx, None, sessions);
        app.offline = true;
        app
    }

    /// An offline shell rooted at `workdir`.
    ///
    /// `None` clears the workspace and the branch so the shell renders the
    /// no-workspace empty state. `Some` re-roots the same fields without
    /// starting an agent session. [`Self::new`] roots itself at the process
    /// working directory, so tests that need either edge case use this seam.
    pub fn with_workspace(
        window: &mut Window,
        cx: &mut Context<Self>,
        workdir: Option<PathBuf>,
    ) -> Self {
        let mut app = Self::build(window, cx, None, Vec::new());
        app.offline = true;
        app.state.branch = workdir.as_deref().and_then(git_branch);
        app.state.workdir = workdir;
        app
    }

    /// A shell seeded with prototype-shaped demo content for design review.
    ///
    /// Selected by `--preview` / `TACT_GUI_PREVIEW`. The rows, plan, diff and
    /// tasks travel through the same rendering path as live data, so what a
    /// reviewer sees here is what an agent session produces — only the source
    /// of the values differs.
    pub fn preview(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut app = Self::build(window, cx, None, preview_sessions());
        app.offline = true;
        app.seed_preview(&mut *cx);
        app
    }

    /// An offline shell holding one pending `ask_user` question.
    ///
    /// The preview seeds the permission shape, whose options answer in a single
    /// press. A question is the multi-select shape, and it is the only one that
    /// renders `Confirm` and `Cancel`; since a request arrives from an agent
    /// session, an offline shell has no other way to put that card on screen.
    /// This is the seam the click tests use to reach those two actions.
    pub fn with_question(
        window: &mut Window,
        cx: &mut Context<Self>,
        prompt: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        let mut app = Self::build(window, cx, None, Vec::new());
        app.offline = true;
        app.state.request = Some(Request {
            id: 1,
            prompt: prompt.into(),
            options,
            multi: true,
            selected: Vec::new(),
        });
        app.sync_transcript_count(&mut *cx);
        app
    }

    /// An offline shell whose thread is one expanded tool card carrying
    /// `output`.
    ///
    /// The preview's own tool rows produce a handful of lines, which fit inside
    /// the card's 150px window; a command that streams more than that is the
    /// only way to reach the block's scroll path, so tests that exercise it
    /// seed a longer body here.
    pub fn with_tool_output(
        window: &mut Window,
        cx: &mut Context<Self>,
        output: impl Into<String>,
    ) -> Self {
        let mut app = Self::build(window, cx, None, Vec::new());
        app.offline = true;
        app.conversation.push_row(transcript::TranscriptRow::Tool {
            display_name: "Running".to_string(),
            detail: "cargo test -p tact".to_string(),
            output: output.into(),
            duration: "1.0s".to_string(),
            status: transcript::ToolStatus::Succeeded,
            expanded: true,
            visual_kind: tact_protocol::ToolVisualKind::Command,
            diff_stats: None,
        });
        app.record_change(Change::Appended(1), cx);
        // The card is the whole thread here, so the shell starts at the top;
        // following the tail is a live session's behaviour.
        app.follow_tail = false;
        app.scroll_transcript_to_top(cx);
        cx.notify();
        app
    }

    /// An offline shell whose thread is one error row carrying `text`.
    ///
    /// An agent error normally arrives through `AgentUpdate::Error`; this seam
    /// puts the same [`transcript::TranscriptRow::Error`] on screen so its
    /// alert semantics can be asserted without a provider.
    pub fn with_error(
        window: &mut Window,
        cx: &mut Context<Self>,
        text: impl Into<String>,
    ) -> Self {
        let mut app = Self::build(window, cx, None, Vec::new());
        app.offline = true;
        app.conversation
            .push_row(transcript::TranscriptRow::Error { text: text.into() });
        app.record_change(Change::Appended(1), cx);
        app.follow_tail = false;
        app.scroll_transcript_to_top(cx);
        cx.notify();
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
        // The prototype reads `Ask permission` in both the composer chip and the
        // status bar, so the preview seeds the protocol's own mode for that
        // behavior. `ask` is not one of the driver's values — it would be read as
        // `auto` — and would leave the chip on its fallback label.
        self.state.permission_mode = "default".to_string();
        self.state.running = true;
        // `.ring` and the status bar's `42% context` chip read the same usage
        // snapshot, so the preview seeds one that draws 42 in both: the ring
        // measures the prompt against `total`, and the chip measures it against
        // the two counts together.
        self.state.usage = Some(tact_protocol::TokenUsageInfo {
            prompt: 4_200,
            completion: 5_800,
            total: 10_000,
            prompt_cache_hit_tokens: 3_200,
            prompt_cache_miss_tokens: 1_000,
            reasoning_tokens: 1_800,
        });
        // The prototype's transcript carries a live permission request. Only a
        // pending request sits past the rows, so the preview seeds one;
        // answering it (there is no agent to hear the answer) files the
        // `.approval.done` card into the transcript as a row.
        self.state.request = Some(Request {
            id: 1,
            prompt: "Run command: cargo check -p tact-gui".to_string(),
            options: vec![
                "Allow once".to_string(),
                "Deny".to_string(),
                "Always allow this tool".to_string(),
            ],
            multi: false,
            selected: Vec::new(),
        });
        // The prototype's sidebar has one active row; the preview runs without
        // an agent, so the newest listed session stands in for the open one.
        self.preview_current = self.recent.first().map(|session| session.id.clone());
        // The worktree group reads the live repository, so pointing the preview
        // at the source tree keeps that group honest instead of hard-coding the
        // prototype's two entries. The manifest directory is used rather than
        // the process working directory: a launcher may start the binary from
        // anywhere, and the preview must not change shape with it.
        let workdir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
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
             ```toml\n\
             theme = \"tact-anthropic\"\n\
             shell = \"sidebar-transcript-workpane\"\n\
             transcript_measure = 720\n\
             ```\n\n\
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
                // The prototype's demo card reads "Thought for 8s".
                duration_seconds: Some(8),
                expanded: true,
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
                visual_kind: tact_protocol::ToolVisualKind::FileRead,
                diff_stats: None,
            });
        self.conversation
            .push_row(transcript::TranscriptRow::Tool {
                display_name: "Running".to_string(),
                detail: "cargo check -p tact-protocol".to_string(),
                output: "Checking tact-protocol v1.1.30\nCompiling serde v1.0.x\nFinished dev profile in 18.4s"
                    .to_string(),
                duration: "18.4s".to_string(),
                status: transcript::ToolStatus::Running,
                expanded: false,
                visual_kind: tact_protocol::ToolVisualKind::Command,
                diff_stats: None,
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
                visual_kind: tact_protocol::ToolVisualKind::FileWrite,
                // The preview's edit result is exactly the three added lines it
                // prints, so the card badges the change the Diff pane holds.
                diff_stats: transcript::diff_line_counts(
                    "+ TactTokens::light()\n+ TactTokens::dark()\n+ ThemeRegistry::watch_dir(\"themes\")",
                ),
            });
        self.record_change(Change::Appended(6), cx);
        // The design review starts at the top of the thread, like the static
        // prototype; a live session keeps follow-tail enabled.
        self.follow_tail = false;
        self.scroll_transcript_to_top(cx);
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
        let transcript_state = cx.new(|cx| MessageScrollerState::new(2, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 5)
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
            files_listed: false,
            diffs: pane::DiffPane::new(),
            conversation: Conversation::default(),
            state: SessionState {
                workdir,
                branch,
                permission_mode: "auto".to_string(),
                ..SessionState::default()
            },
            queued: VecDeque::new(),
            toasts: Vec::new(),
            attachments: Vec::new(),
            transcript_state,
            composer,
            session_search,
            session,
            recent,
            preview_current: None,
            offline: false,
            _pump: pump,
            palette: None,
            palette_live: false,
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

    /// The session the window has open, for tests and status surfaces.
    ///
    /// Unlike [`Self::session_id`] this answers in an offline shell too, where
    /// the open row is whichever one a click picked.
    pub fn open_session(&self) -> Option<&str> {
        self.open_session_id()
    }

    /// The sidebar's session ids in display order, for tests and status surfaces.
    pub fn recent_row_ids(&self) -> Vec<String> {
        self.recent.iter().map(|row| row.id.clone()).collect()
    }

    /// Whether a turn is currently in flight.
    pub fn is_running(&self) -> bool {
        self.state.running
    }

    /// Number of transcript rows, for tests and status surfaces.
    pub fn transcript_len(&self) -> usize {
        self.conversation.len()
    }

    /// The current message draft, for tests and command surfaces.
    pub fn composer_draft(&self, cx: &App) -> String {
        self.composer.read(cx).value().to_string()
    }

    /// Open the Tact command palette over the current window.
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        self.palette = Some(state.clone());
        self.palette_live = true;
        let dialog_state = state.clone();
        let owner = cx.weak_entity();

        window.open_dialog(cx, move |dialog, _window, _cx| {
            let dialog_state = dialog_state.clone();
            let owner = owner.clone();
            dialog
                // The prototype's palette is chrome-less: its first row is the
                // search field, with the dismissal legend in `.pfoot` and the
                // `esc`/backdrop paths covering what a close button would do.
                .close_button(false)
                .width(px(560.))
                .content(move |content, _window, _cx| {
                    let owner = owner.clone();
                    content.child(
                        commands::groups().into_iter().fold(
                            Command::new(&dialog_state)
                                .placeholder("Search commands, sessions, files…")
                                // `.pfoot`: the keyboard legend under the list.
                                .footer(|_, _, cx| palette_footer(crate::theme::ink3(cx)))
                                // Every item carries its own GPUI action, and
                                // `CommandState::confirm` dispatches it before
                                // this callback runs. Running the command here
                                // too would execute every palette row twice —
                                // a no-op for idempotent rows like Open diff,
                                // but a silent double toggle for sidebar, work
                                // pane, theme, and session cycling.
                                //
                                // The callback also arrives one frame late, so
                                // it must not pop a dialog a row opened on top
                                // of the palette. `palette_live` is the shell's
                                // record that the palette is still the topmost
                                // dialog; the Open settings row clears it after
                                // closing the palette itself.
                                .on_confirm({
                                    let owner = owner.clone();
                                    move |_, window, cx| {
                                        let still_open = owner
                                            .update(cx, |app, _| {
                                                std::mem::take(&mut app.palette_live)
                                            })
                                            .unwrap_or(false);
                                        if still_open {
                                            window.close_dialog(cx);
                                        }
                                    }
                                })
                                // Escape, the cancel chord, and the backdrop
                                // all leave through this hook; clearing the
                                // flag here keeps a dismissed palette from
                                // making some later dialog look like a row's
                                // replacement for it.
                                .on_cancel({
                                    let owner = owner.clone();
                                    move |_, cx| {
                                        let _ = owner.update(cx, |app, _| app.palette_live = false);
                                    }
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

    /// Execute one command.
    ///
    /// Every action handler below funnels through here, so a palette row and
    /// its keyboard chord cannot drift apart: both dispatch the same GPUI
    /// action, and that action lands on exactly one arm of this table.
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
            PaletteCommand::FocusComposer => {
                // A palette row runs this while its palette is still open, and
                // closing a dialog gives focus back to whatever held it before
                // the dialog opened — which would undo a focus handed over
                // before the pop. Close the palette first (the same LIFO
                // reason as `OpenSettings`), then focus; `palette_live` keeps
                // the palette's own late confirmation from popping anything.
                // On the chord path no palette is open and only the focus runs.
                if std::mem::take(&mut self.palette_live) {
                    window.close_dialog(cx);
                }
                self.focus_composer(window, cx);
            }
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
            PaletteCommand::OpenSettings => {
                // A palette row runs this while its palette is still open, and
                // `Root::close_dialog` pops the top of a LIFO stack: opening
                // settings first would let the palette's own (one frame late)
                // confirmation pop the settings dialog instead. Pop the
                // palette here, in the same dispatch, and open settings after
                // it; `palette_live` then tells that late confirmation to
                // leave the stack alone. On the chord path no palette is open
                // and this is a single `open_dialog`.
                if std::mem::take(&mut self.palette_live) {
                    window.close_dialog(cx);
                }
                self.open_settings(window, cx);
            }
            PaletteCommand::ToggleTheme => {
                theme::toggle(window, cx);
                let note = theme_toast(cx);
                push_toast(window, cx, note);
            }
        }
    }

    /// Start a fresh agent session in the current workspace.
    fn new_session(&mut self, cx: &mut Context<Self>) {
        // The offline shell owns no runtime, so this control says so instead of
        // reaching for a session it cannot start.
        if self.offline {
            self.push_system_row(
                "This shell runs without an agent, so there is no session to start.".into(),
                cx,
            );
            return;
        }

        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };

        match session::start(workdir.clone()) {
            Ok((handle, streams)) => self.adopt(workdir, handle, streams, cx),
            Err(err) => self.push_system_row(format!("Could not start a session: {err:#}"), cx),
        }
    }

    /// Re-root the window's workspace at another git worktree.
    ///
    /// The worktree group is where the window's workspace is chosen, so a click
    /// moves the branch line and the panes that read the live repository, and —
    /// in a connected window — the sessions the sidebar lists. An attached
    /// agent session keeps its own root: the window re-scopes what it shows
    /// without restarting the session, which also means the next new session
    /// starts in the worktree the user picked.
    fn switch_worktree(&mut self, worktree: PathBuf, cx: &mut Context<Self>) {
        if self.state.workdir.as_deref() == Some(worktree.as_path()) {
            return;
        }
        self.state.branch = git_branch(&worktree);
        self.state.workdir = Some(worktree.clone());
        // The list the window shows belongs to the workspace it just left, so a
        // connected window swaps in the new worktree's own sessions. An offline
        // shell owns no store: its rows are demo data handed to the
        // constructor, and re-rooting must not replace them with whatever the
        // filesystem happens to hold.
        if !self.offline {
            self.recent = session::recent(&worktree);
            self.preview_current = self.recent.first().map(|session| session.id.clone());
        }
        // Both panes are read from the workspace, and both cache what they read.
        self.files.invalidate();
        self.diffs.invalidate();
        cx.notify();
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
        // An offline shell has no runtime to resume into and its rows are demo
        // data, so a click moves the row the sidebar marks as open — the
        // prototype's `.row.active` — rather than starting an agent.
        if self.offline {
            if self.recent.iter().any(|row| row.id == session_id) {
                self.preview_current = Some(session_id);
                cx.notify();
            }
            return;
        }
        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };

        match session::resume(workdir.clone(), session_id.clone()) {
            Ok((handle, streams)) => {
                let short = session::short_id(&session_id).to_string();
                self.adopt(workdir.clone(), handle, streams, cx);
                self.replay_history(&workdir, &session_id, cx);
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

    /// Whether the open session is archived, for the menu's archive row.
    fn open_session_archived(&self) -> bool {
        self.open_row().is_some_and(|row| row.archived)
    }

    /// The sidebar row the shell has open, when it still lists one.
    fn open_row(&self) -> Option<&RecentSession> {
        let id = self.open_session_id()?;
        self.recent.iter().find(|row| row.id == id)
    }

    /// Ask for a new name for the open session.
    ///
    /// The name lives in the store's `title` column, so the dialog is the only
    /// way to set one; a session with no name keeps the label its opening
    /// message gives it, and emptying the field goes back to that label.
    fn open_rename_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.open_session_id().map(str::to_string) else {
            self.push_system_row("No session is open to rename.".into(), cx);
            return;
        };
        let current = match self.open_row() {
            Some(row) => session_row_title(row),
            None => session::short_id(&session_id).to_string(),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(current)
                .placeholder("Session name")
        });
        let owner = cx.weak_entity();
        // The deferred focus below runs after the dialog opens, so it keeps its
        // own handle on the field.
        let focus_input = input.clone();

        window.open_dialog(cx, move |dialog, _window, _cx| {
            // The dialog's content builder runs on every frame, so it takes its
            // own handle on the field rather than borrowing this one.
            let content_input = input.clone();
            let commit_owner = owner.clone();
            let commit_input = input.clone();
            let commit_id = session_id.clone();
            // Enter arrives here through the dialog's own key context; the
            // footer's Rename button dispatches the same confirm, so both paths
            // commit once.
            let confirm = move |_: &mut Window, cx: &mut App| {
                let name = commit_input.read(cx).value().to_string();
                let _ = commit_owner.update(cx, |app, cx| app.rename_session(&commit_id, name, cx));
            };
            let enter_owner = owner.clone();
            let enter_input = input.clone();
            let enter_id = session_id.clone();
            dialog
                .title("Rename session")
                .width(px(420.))
                .on_ok(move |_, _window, cx| {
                    let name = enter_input.read(cx).value().to_string();
                    let _ =
                        enter_owner.update(cx, |app, cx| app.rename_session(&enter_id, name, cx));
                    true
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("session-rename-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("session-rename-ok").label("Rename").on_click(
                            move |_, window, cx| {
                                confirm(window, cx);
                                window.close_dialog(cx);
                            },
                        )),
                )
                .content(move |content, _window, _cx| {
                    // The field carries the id through a wrapper: `Input` is a
                    // render-once element without a test-support id of its own.
                    content.child(
                        div()
                            .id("session-rename-input")
                            .test_support()
                            .w_full()
                            .child(Input::new(&content_input)),
                    )
                })
        });

        window.defer(cx, move |window, cx| {
            focus_input.update(cx, |state, cx| state.focus(window, cx));
        });
    }

    /// Store a new name for `session_id`, then redraw the sidebar from it.
    ///
    /// The offline preview owns no store, so it renames its own row instead --
    /// the same edit the prototype's script makes to the DOM -- and a connected
    /// shell writes the column and reloads the list.
    fn rename_session(&mut self, session_id: &str, name: String, cx: &mut Context<Self>) {
        let trimmed = name.trim().to_string();
        if self.offline {
            if let Some(row) = self.recent.iter_mut().find(|row| row.id == session_id) {
                row.name = (!trimmed.is_empty()).then(|| trimmed.clone());
                self.announce_rename(session_id, &trimmed, cx);
            }
            return;
        }
        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };
        match session::rename(&workdir, session_id, &trimmed) {
            Ok(()) => {
                self.recent = session::recent(&workdir);
                self.announce_rename(session_id, &trimmed, cx);
            }
            Err(err) => self.push_system_row(format!("Could not rename the session: {err:#}"), cx),
        }
    }

    /// The transcript's record of a rename, whichever store it went to.
    fn announce_rename(&mut self, session_id: &str, name: &str, cx: &mut Context<Self>) {
        let short = session::short_id(session_id).to_string();
        let text = if name.is_empty() {
            format!("Cleared the name of session {short}; it shows its opening words again.")
        } else {
            format!("Renamed session {short} to \"{name}\".")
        };
        self.push_system_row(text, cx);
    }

    /// Copy the open session's conversation into a new one, then open the copy.
    fn duplicate_open_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.open_session_id().map(str::to_string) else {
            self.push_system_row("No session is open to duplicate.".into(), cx);
            return;
        };
        let source_short = session::short_id(&session_id).to_string();

        if self.offline {
            let Some(source) = self.recent.iter().find(|row| row.id == session_id).cloned() else {
                return;
            };
            let copy = RecentSession {
                id: format!("{}-copy-{}", source.id, session::now_unix()),
                updated_at_unix: session::now_unix(),
                name: Some(format!("{} (copy)", session_row_title(&source))),
                archived: false,
                ..source
            };
            let copy_id = copy.id.clone();
            self.recent.insert(0, copy);
            self.preview_current = Some(copy_id.clone());
            self.push_system_row(
                format!(
                    "Duplicated session {source_short} into {}: the copy keeps the conversation and starts from it.",
                    session::short_id(&copy_id)
                ),
                cx,
            );
            return;
        }

        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };
        match session::duplicate(&workdir, &session_id) {
            Ok(copy_id) => {
                self.recent = session::recent(&workdir);
                // Open the copy: pressing Duplicate is about continuing from
                // here, not about leaving a row behind to find later.
                self.resume_session(copy_id.clone(), cx);
                self.push_system_row(
                    format!(
                        "Duplicated session {source_short} into {}. The copy carries the conversation and no provider state, so its next turn replays it.",
                        session::short_id(&copy_id)
                    ),
                    cx,
                );
            }
            Err(err) => {
                self.push_system_row(format!("Could not duplicate the session: {err:#}"), cx)
            }
        }
    }

    /// Archive or restore the open session.
    ///
    /// Archiving is a flag, never a delete: the row and its transcript stay in
    /// the store and in the sidebar, carrying the prototype's `Archived` badge
    /// until the flag is cleared.
    fn set_open_session_archived(&mut self, archived: bool, cx: &mut Context<Self>) {
        let Some(session_id) = self.open_session_id().map(str::to_string) else {
            self.push_system_row("No session is open to archive.".into(), cx);
            return;
        };
        if self.offline {
            if let Some(row) = self.recent.iter_mut().find(|row| row.id == session_id) {
                row.archived = archived;
                self.announce_archive(&session_id, archived, cx);
            }
            return;
        }
        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };
        match session::set_archived(&workdir, &session_id, archived) {
            Ok(()) => {
                self.recent = session::recent(&workdir);
                self.announce_archive(&session_id, archived, cx);
            }
            Err(err) => self.push_system_row(format!("Could not archive the session: {err:#}"), cx),
        }
    }

    /// The transcript's record of an archive, whichever store it went to.
    fn announce_archive(&mut self, session_id: &str, archived: bool, cx: &mut Context<Self>) {
        let short = session::short_id(session_id).to_string();
        let text = if archived {
            format!("Archived session {short}; it keeps its transcript and can be restored.")
        } else {
            format!("Restored session {short}.")
        };
        self.push_system_row(text, cx);
    }

    /// Hand the workspace to the desktop's file manager.
    ///
    /// The offline preview does not launch one -- a preview or a test run must
    /// not open windows behind the user -- so it says what it would open rather
    /// than pretending it did.
    fn reveal_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(workdir) = self.workspace_dir() else {
            self.push_system_row("Cannot determine the workspace directory.".into(), cx);
            return;
        };
        if self.offline {
            self.push_system_row(
                format!(
                    "The workspace is {}; the offline preview does not open a file manager.",
                    workdir.display()
                ),
                cx,
            );
            return;
        }
        match session::reveal(&workdir) {
            Ok(()) => self.push_system_row(
                format!("Opened {} in the file manager.", workdir.display()),
                cx,
            ),
            Err(err) => self.push_system_row(format!("Could not open the workspace: {err:#}"), cx),
        }
    }

    /// Redraw a reopened session's stored transcript.
    ///
    /// Switching sessions used to hand the window a blank page while the store
    /// still held the conversation; this redraws it. An unreadable or empty
    /// history leaves the blank page the shell would have shown anyway, so a
    /// resume never fails over a transcript that cannot be replayed.
    fn replay_history(
        &mut self,
        workdir: &std::path::Path,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let messages = session::history(workdir, session_id);
        if messages.is_empty() {
            return;
        }
        self.conversation.load_history(&messages);
        // The rows arrived in one go, so the scroller is resized rather than
        // appended to one at a time. `follow_tail` then lands on the newest
        // turn, which is what a reopened thread should show.
        self.record_change(Change::None, cx);
        cx.notify();
    }

    /// The session after the current one in sidebar order, wrapping at the end.
    ///
    /// This is what makes the keyboard contract complete: `Ctrl+Tab` reaches
    /// every session in the workspace without a pointer.
    fn next_session_id(&self) -> Option<String> {
        if self.recent.is_empty() {
            return None;
        }
        let current = self.open_session_id();
        let position = current.and_then(|id| self.recent.iter().position(|row| row.id == id));
        let next = next_session_index(self.recent.len(), position)?;
        self.recent.get(next).map(|row| row.id.clone())
    }

    /// The session's own name, as `.head h1` shows it.
    fn session_name(&self) -> Option<String> {
        session_name(
            &self.recent,
            self.open_session_id(),
            self.conversation.rows(),
        )
    }

    /// The session this window shows: the attached one, or the preview's row.
    fn open_session_id(&self) -> Option<&str> {
        self.session
            .as_ref()
            .map(SessionHandle::session_id)
            .or(self.preview_current.as_deref())
    }

    /// The session before the current one in sidebar order, wrapping at the start.
    fn previous_session_id(&self) -> Option<String> {
        if self.recent.is_empty() {
            return None;
        }
        let current = self.open_session_id();
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
            .update(cx, |state, cx| state.reset(2, cx));
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

    /// Copy the transcript to the system clipboard as Markdown.
    ///
    /// The prototype's `.mainTop` puts a copy icon beside the detail cycle;
    /// this is the behaviour behind it.
    fn copy_transcript(&mut self, cx: &mut Context<Self>) {
        let markdown = self.conversation.to_markdown();
        if markdown.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(markdown));
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
        self.run_palette_command(PaletteCommand::OpenPalette, window, cx);
    }

    fn on_new_session(
        &mut self,
        _: &commands::NewSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::NewSession, window, cx);
    }

    /// Escape stops the running task -- unless the work pane is the drawer the
    /// prototype's Escape dismisses first.
    ///
    /// `docs/design/tact-desktop-prototype.html:159` runs
    /// `if(overlay.open) closePalette(); else if(body.workOpen) work(false)`.
    /// The palette half needs nothing here: a focused dialog carries its own
    /// `Dialog` key context, which outranks this one, so Escape is its `Cancel`
    /// before the shell is ever consulted. The drawer half is this: while the
    /// pane floats over the transcript, the key dismisses the overlay in front
    /// of the user. Where the pane has a column of its own it keeps its v1
    /// meaning -- Escape stops the task -- because tearing a persistent column
    /// out of the layout is not what Escape is for here.
    fn on_stop_task(
        &mut self,
        _: &commands::StopTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.work_pane_is_drawer(window) {
            self.close_work_pane();
            cx.notify();
            return;
        }
        self.run_palette_command(PaletteCommand::StopTask, window, cx);
    }

    fn on_toggle_work_pane(
        &mut self,
        _: &commands::ToggleWorkPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::ToggleWorkPane, window, cx);
    }

    fn on_focus_composer(
        &mut self,
        _: &commands::FocusComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::FocusComposer, window, cx);
    }

    fn on_cycle_detail(
        &mut self,
        _: &commands::CycleTranscriptDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::CycleTranscriptDetail, window, cx);
    }

    fn on_toggle_sidebar(
        &mut self,
        _: &commands::ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::ToggleSidebar, window, cx);
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
        self.run_palette_command(PaletteCommand::OpenSettings, window, cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::CompactSession, window, cx);
    }

    fn on_session_stats(
        &mut self,
        _: &commands::SessionStats,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::SessionStats, window, cx);
    }

    fn on_mcp_servers(
        &mut self,
        _: &commands::McpServers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::McpServers, window, cx);
    }

    fn on_toggle_theme(
        &mut self,
        _: &commands::ToggleTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_palette_command(PaletteCommand::ToggleTheme, window, cx);
    }

    /// Select a work pane tab.
    pub(crate) fn set_work_pane(&mut self, pane: WorkPane) {
        self.work_pane = pane;
    }

    /// Select a work pane the way its own tab does, opening the pane when this
    /// width only has the drawer form.
    ///
    /// The prototype's `pane(n)` ends with `if(innerWidth<=1120) work(true)`:
    /// on a narrow window, selecting a pane is also asking to see it, because
    /// there the pane has no column to be shown in. Without this a preset press
    /// below the drawer threshold changed the pane behind a closed drawer and
    /// nothing on screen moved -- the same failure the floating sidebar had.
    pub(crate) fn select_work_pane(&mut self, pane: WorkPane, window: &Window) {
        self.set_work_pane(pane);
        if window.bounds().size.width < WORK_PANE_IN_FLOW_FROM.to_pixels(window.rem_size()) {
            self.work_pane_open = true;
        }
    }

    /// Close the work pane, as the `.workTop` close button does.
    pub(crate) fn close_work_pane(&mut self) {
        self.work_pane_open = false;
    }

    /// Whether the work pane is rendering as the prototype's overlay drawer.
    ///
    /// Below `WORK_PANE_IN_FLOW_FROM` the open pane has no column to sit in, so
    /// it floats over the transcript as a drawer. That is the form `escape`
    /// dismisses, and the form `select_work_pane` has to bring back.
    fn work_pane_is_drawer(&self, window: &Window) -> bool {
        self.work_pane_open
            && window.bounds().size.width < WORK_PANE_IN_FLOW_FROM.to_pixels(window.rem_size())
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

    pub fn scroll_transcript_to_end(&mut self, cx: &mut Context<Self>) {
        self.follow_tail = true;
        self.transcript_state
            .update(cx, |state, cx| state.scroll_to_end(cx));
        cx.notify();
    }

    /// Bring the virtual transcript's header into view.
    ///
    /// The header is the first item in the same virtual list as the rows, so
    /// tests and future search jumps need an explicit start-of-thread anchor.
    pub fn scroll_transcript_to_top(&mut self, cx: &mut Context<Self>) {
        self.follow_tail = false;
        self.transcript_state
            .update(cx, |state, cx| state.scroll_to_item(0, cx));
        cx.notify();
    }

    /// Bring one transcript row into view.
    ///
    /// The transcript is a virtual list, so rows outside the viewport are not
    /// built at all. Anything that needs a specific row (a search jump, or a
    /// test asserting that row's geometry) scrolls it in through here.
    pub fn scroll_transcript_to(&mut self, index: usize, cx: &mut Context<Self>) {
        self.follow_tail = false;
        self.transcript_state
            .update(cx, |state, cx| state.scroll_to_item(index + 1, cx));
        cx.notify();
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
            self.toast(Notification::success("Queued follow-up"), cx);
            return;
        }

        self.dispatch(submitted, cx);
        cx.notify();
    }

    /// Queue a toast for the next render.
    fn toast(&mut self, note: Notification, cx: &mut Context<Self>) {
        self.toasts.push(note);
        cx.notify();
    }

    /// Insert the `@` file-mention trigger at the end of the draft.
    ///
    /// The composer only opens file completion for a trigger at the start of
    /// the input or after whitespace, so a draft that ends in a word gets the
    /// separating space first.
    fn insert_mention(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.composer.read(cx).value().to_string();
        let value = if draft.is_empty() || draft.ends_with(char::is_whitespace) {
            format!("{draft}@")
        } else {
            format!("{draft} @")
        };
        self.composer
            .update(cx, |state, cx| state.set_value(value, window, cx));
        self.focus_composer(window, cx);
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
        let cancelled = matches!(update, tact_protocol::AgentUpdate::TaskCancelled);
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
        if cancelled {
            self.toast(Notification::success("Task cancelled"), cx);
        }
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

    /// The virtual transcript contains the session header, the conversation
    /// rows, and at most one *pending* card.
    ///
    /// An answered card is a conversation row, so it is counted by `rows`; only
    /// a request the agent is still waiting on sits outside the list, at the
    /// tail where the answer has to be given.
    fn transcript_item_count(&self) -> usize {
        let rows = self.conversation.len();
        let extra = usize::from(self.state.request.is_some());
        let empty = usize::from(rows == 0 && extra == 0);
        1 + rows + extra + empty
    }

    /// Keep the virtual list's item count aligned with the transcript chrome.
    ///
    /// Appending a conversation row changes the count; request lifecycle
    /// changes add or retain the inline approval row. A reset is only used
    /// when that shape changes, not for ordinary row remeasurement.
    fn sync_transcript_count(&mut self, cx: &mut Context<Self>) {
        let desired = self.transcript_item_count();
        self.transcript_state.update(cx, |state, cx| {
            if state.item_count() != desired {
                state.reset(desired, cx);
            }
        });
    }

    fn record_change(&mut self, change: Change, cx: &mut Context<Self>) {
        match change {
            Change::None => {}
            Change::Appended(count) => {
                self.transcript_state
                    .update(cx, |state, cx| state.append(count, cx));
            }
            Change::Resized(index) => {
                self.transcript_state.update(cx, |state, cx| {
                    let index = index + 1;
                    state.remeasure_items(index..index + 1, cx)
                });
            }
        }
        self.sync_transcript_count(cx);
        if self.follow_tail {
            self.transcript_state
                .update(cx, |state, cx| state.scroll_to_end(cx));
        }
    }

    /// Append a local system row (startup, session-level notices, and the
    /// work pane's prototype-only actions).
    pub(crate) fn push_system_row(&mut self, text: String, cx: &mut Context<Self>) {
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
        // The card reports the choice by its own label, so a permission answer
        // reads `Allow once` rather than a re-invented phrase.
        let result = request
            .options
            .get(index)
            .cloned()
            .unwrap_or_else(|| "Answered".to_string());
        self.answer(
            UiResponse::Select {
                request_id: request.id,
                choice: Some(index),
            },
            result,
            cx,
        );
    }

    /// Confirm a multi-select request.
    fn confirm_multi(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.state.request.clone() else {
            return;
        };
        let result = format!(
            "Confirmed {} choice{}",
            request.selected.len(),
            if request.selected.len() == 1 { "" } else { "s" }
        );
        self.answer(
            UiResponse::MultiSelect {
                request_id: request.id,
                choices: Some(request.selected.clone()),
            },
            result,
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
        self.answer(response, "Dismissed".to_string(), cx);
    }

    /// Send a response and settle the prompt so it cannot be answered twice.
    ///
    /// The answered card moves into the transcript as a row, which is what
    /// keeps it from being the last thing on screen once the turn it unblocked
    /// resumes: only the pending slot is dropped, so the card keeps the slot it
    /// was asked in.
    fn answer(&mut self, response: UiResponse, result: String, cx: &mut Context<Self>) {
        if let Some(session) = self.session.as_ref() {
            session.send(tact_protocol::UserCommand::UiResponse(response));
        }
        if let Some(request) = self.state.request.take() {
            self.conversation.push_approval(request, result);
            self.record_change(Change::Appended(1), cx);
        }
        self.sync_transcript_count(cx);
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
        // Deliver anything the session pump queued while no window was in hand.
        for note in std::mem::take(&mut self.toasts) {
            push_toast(window, cx, note);
        }
        let width = window.bounds().size.width;
        let rem_size = window.rem_size();
        // The Files pane caches its `read_dir` walk, so it has to be told when
        // it re-enters the screen; otherwise a listing taken before the agent
        // wrote a file would survive for the rest of the process.
        let files_visible = self.work_pane_open && matches!(self.work_pane, WorkPane::Files);
        if files_visible && !self.files_listed {
            self.files.invalidate();
        }
        self.files_listed = files_visible;
        let columns = Columns::for_width(width, rem_size);
        let sidebar_is_overlay = width < SIDEBAR_OVERLAY_UNDER.to_pixels(rem_size);
        // The sidebar is a real column only when it is both open and wide
        // enough to sit beside the transcript. The title bar reserves the
        // column's width on this same condition, so closing the sidebar at a
        // wide width does not leave the header holding a gap the body dropped.
        let sidebar_in_flow = !sidebar_is_overlay && self.sidebar_open;
        let work_pane_is_drawer = self.work_pane_is_drawer(window);
        let work_pane_in_flow = self.work_pane_open && !work_pane_is_drawer;
        // The drawer *form* is a function of the width alone. Whether it is on
        // screen is `work_pane_open`, which the slide samples -- so the two
        // have to be asked separately: the width decides what to sample, the
        // flag decides where the slide is in its travel.
        let work_pane_drawer_form = width < WORK_PANE_IN_FLOW_FROM.to_pixels(rem_size);

        // `.head h1` carries the session's own name; the title-bar chip echoes
        // it on Chat and takes the preset's name on Agent and Code, which is
        // what the prototype's `.tab` handler writes into `#sessionName`. A
        // session with no user turn yet has no name, so both fall back to the
        // project and preset the window is actually showing.
        let preset_heading = format!(
            "{} · {} workspace",
            project_label(&self.state),
            self.workspace.label()
        );
        let session_heading = SharedString::from(
            self.session_name()
                .unwrap_or_else(|| preset_heading.clone()),
        );
        let chip_heading = match self.workspace {
            Workspace::Chat => session_heading.clone(),
            preset => SharedString::from(format!("{} workspace", preset.label())),
        };
        let transcript_frame = TranscriptFrame {
            header: TranscriptHeader {
                heading: session_heading.clone(),
                subtitle: self.preview_current.as_ref().map(|_| {
                    SharedString::from(
                        "Turn Direction A into a clickable prototype with Anthropic tokens and gpui-kit component boundaries.",
                    )
                }),
                detail: self.detail,
            },
            columns,
        };

        let mut workspace_row = h_flex().size_full().min_h_0().items_stretch();
        if sidebar_in_flow {
            workspace_row = workspace_row.child(sidebar(
                &self.state,
                &self.recent,
                self.open_session_id(),
                &self.session_search,
                columns.sidebar,
                cx,
            ));
        }
        workspace_row = workspace_row.child(transcript(
            self.conversation.rows(),
            self.transcript_state.clone(),
            self.composer.clone(),
            &self.state,
            &self.attachments,
            transcript_frame,
            cx,
        ));

        if work_pane_in_flow {
            workspace_row = workspace_row.child(work_pane(
                self.work_pane,
                &self.state,
                &mut self.files,
                &mut self.diffs,
                columns.work_pane,
                cx,
            ));
        }

        let mut workspace = div().relative().flex_1().min_h_0().child(workspace_row);
        // The three floating surfaces are painted in the prototype's own
        // stacking order -- `.scrim` at `z-index:25`, `.work` at `30`, the
        // floating `.sidebar` at `40` -- because GPUI has no z-index and paint
        // order is what hit testing walks. The scrim therefore dims the
        // workspace under both, and the sidebar still wins where a narrow
        // window makes the sidebar and the drawer overlap.
        // `.scrim` only toggles `display` -- the prototype declares it inside
        // its own narrow block as `body.workOpen .scrim{...display:block}` --
        // so it appears and disappears on the frame the pane does, with no
        // transition of its own, and the drawer slides out un-dimmed.
        if work_pane_is_drawer {
            workspace = workspace.child(work_pane_scrim(cx));
        }
        // `.work` and the floating `.sidebar` are the two surfaces the
        // prototype gives `transition:transform 180ms`. Both therefore stay in
        // the tree for the length of the exit, which is why the mount is gated
        // on the width-only form and the open flag goes to `Presence` instead:
        // `should_render` keeps the panel alive while it slides away, and
        // drops it on the frame the transition finishes.
        if work_pane_drawer_form {
            let slide = Presence::new(WORK_PANE_DRAWER_PRESENCE, self.work_pane_open)
                .transition(overlay_slide())
                .sample(window, cx);
            if slide.should_render() {
                workspace = workspace.child(work_pane_drawer(
                    self.work_pane,
                    &self.state,
                    &mut self.files,
                    &mut self.diffs,
                    slide.progress,
                    cx,
                ));
            }
        }
        if sidebar_is_overlay {
            let slide = Presence::new(SIDEBAR_OVERLAY_PRESENCE, self.sidebar_open)
                .transition(overlay_slide())
                .sample(window, cx);
            if slide.should_render() {
                workspace = workspace.child(sidebar_overlay(
                    &self.state,
                    &self.recent,
                    // The same `current` the column gets: an offline shell marks
                    // its open row through `preview_current`, so passing only
                    // `self.session` left the floating sidebar with no active row
                    // and a session press that looked like it did nothing.
                    self.open_session_id(),
                    &self.session_search,
                    slide.progress,
                    cx,
                ));
            }
        }

        v_flex()
            .id("tact-app")
            .test_support()
            .size_full()
            .track_focus(&self.root_focus)
            .key_context(commands::CONTEXT)
            .bg(cx.theme().popover)
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
                TitleBarState {
                    workspace: self.workspace,
                    sidebar_open: self.sidebar_open,
                    work_pane_open: self.work_pane_open,
                    session_heading: chip_heading,
                    compact: width < px(1320.),
                    // `SIDEBAR_OVERLAY_UNDER` and `WORK_PANE_IN_FLOW_FROM`: a
                    // segment reserves its column's width only while the shell
                    // really lays that column out, which also covers a panel
                    // the user has closed.
                    sidebar_float: !sidebar_in_flow,
                    work_pane_float: !work_pane_in_flow,
                    columns,
                    open_archived: self.open_session_archived(),
                },
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

/// The title bar's session chip: a list glyph, the open session's name, and the
/// disclosure chevron.
///
/// It is the trigger of the session dropdown, which is why it is a named type:
/// [`Popover`] takes a [`Selectable`] trigger, and a bare `div` cannot implement
/// that trait. The box is the prototype's `.session` -- a 7 px inline pad, a
/// 4 px block pad, a 6 px radius, and a 12.5 px label truncated at 240 px -- so
/// it keeps its own metrics rather than borrowing [`MiniTrigger`]'s 25 px
/// `.mini`, which the title bar has no room for.
#[derive(IntoElement)]
struct SessionChip {
    /// The name the chip shows, truncated to the prototype's 240 px cap.
    label: SharedString,
    /// Whether the dropdown is open. An open menu paints the chip exactly as
    /// the pointer does, so the control reads as held while its rows show.
    selected: bool,
}

impl SessionChip {
    fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            selected: false,
        }
    }
}

impl Selectable for SessionChip {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for SessionChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { label, selected } = self;
        let hover = cx.theme().accent;
        let ink = cx.theme().foreground;
        h_flex()
            .id("session-menu")
            .test_support()
            .min_w_0()
            .max_w(rems(15.))
            .items_center()
            .gap(rems(0.4375))
            .px(rems(0.4375))
            .py(rems(0.25))
            .rounded(rems(0.375))
            .text_color(cx.theme().muted_foreground)
            .when(selected, |this| this.bg(hover).text_color(ink))
            .hover(move |style| style.bg(hover).text_color(ink))
            .child(IconName::MessageSquareText)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(0.78125))
                    .text_color(ink)
                    .child(label),
            )
            .child(Icon::from(IconName::ChevronDown).with_size(px(13.)))
    }
}

/// Everything the title bar reads from the shell for one frame.
///
/// The responsive flags travel together: `compact` drops the palette chord,
/// `sidebar_float` drops the sidebar column's reserved width whenever the shell
/// stops laying that column out -- the sidebar floats over the transcript or is
/// closed -- and `work_pane_float` does the same for the work-pane column once
/// the pane is a drawer or hidden. Keeping them in one value stops the call site
/// from growing a positional argument per breakpoint.
struct TitleBarState {
    workspace: Workspace,
    sidebar_open: bool,
    work_pane_open: bool,
    session_heading: SharedString,
    compact: bool,
    sidebar_float: bool,
    work_pane_float: bool,
    /// `.top` shares the shell's `grid-template-columns`, so the chrome's two
    /// side segments reserve whatever the columns below them actually take.
    columns: Columns,
    /// Whether the open session is archived. The dropdown's archive row has to
    /// read "Unarchive" for a session that is already archived, or the flag
    /// would be a one-way door in the menu.
    open_archived: bool,
}

fn title_bar(state: TitleBarState, cx: &mut Context<TactApp>) -> impl IntoElement {
    let TitleBarState {
        workspace,
        sidebar_open,
        work_pane_open,
        session_heading,
        compact,
        sidebar_float,
        work_pane_float,
        columns,
        open_archived,
    } = state;
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

    // `.tabs` / `.tab`: a 2px-padded segmented strip on `--surface2`, ringed by
    // a 1px `--line` border, whose 26px tabs take `--surface` when active. The
    // stock `TabBar::segmented()` is 32px tall on a `--line`-filled bar with
    // `--page` inside and exposes no per-tab box override, so the strip is
    // composed here. Every tab keeps the integer id that
    // `within("workspace-tabs")` selects by.
    let tab_hover = cx.theme().accent;
    let tab_ink = cx.theme().foreground;
    let tab_muted = cx.theme().muted_foreground;
    let tab_line = cx.theme().border;
    let tabs = h_flex()
        .id("workspace-tabs")
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .border_1()
        .border_color(tab_line)
        .rounded(rems(0.5625))
        .bg(cx.theme().muted)
        .children(
            Workspace::ALL
                .iter()
                .copied()
                .enumerate()
                .map(|(index, preset)| {
                    let active = index == workspace.index();
                    h_flex()
                        .id(index)
                        .items_center()
                        .h(px(26.))
                        .px(rems(0.6875))
                        // `.tab.active` draws its `0 0 0 1px var(--line)` ring;
                        // the transparent border keeps every tab the same box.
                        .border_1()
                        .border_color(if active {
                            tab_line
                        } else {
                            cx.theme().transparent
                        })
                        .rounded(rems(0.375))
                        .when(active, |tab| {
                            // `.tab.active{box-shadow:0 1px 2px rgba(20,20,19,.06),
                            // 0 0 0 1px var(--line)}`: the ring is the border
                            // above, the lift is the prototype's own ink, which
                            // stays dark in both themes rather than flipping
                            // with `foreground`.
                            tab.bg(cx.theme().popover).shadow(vec![
                                gpui_kit::gpui::BoxShadow::new(
                                    px(0.),
                                    px(1.),
                                    gpui_kit::Hsla::from(gpui_kit::rgba(0x1414_13ff)).opacity(0.06),
                                )
                                .blur_radius(px(2.)),
                            ])
                        })
                        .text_size(rems(0.75))
                        .font_semibold()
                        .text_color(if active { tab_ink } else { tab_muted })
                        // `.tab.active` wins over `.tab:hover` in the sheet, so
                        // only the inactive tabs answer the pointer.
                        .when(!active, |tab| {
                            tab.hover(move |style| style.bg(tab_hover).text_color(tab_ink))
                        })
                        // `.tab` is a `<button>` in the prototype. Focusing an
                        // active tab replaces its `0 1px 2px` lift with the
                        // ring, since a refined style swaps the whole shadow
                        // list; the lift is not part of the ring contract.
                        .tab_index(0)
                        .focus_visible({
                            let ring = focus_visible_ring(cx);
                            move |style| style.shadow(ring.clone())
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // Selecting a preset also selects its work pane,
                            // exactly as
                            // `pane(n==='agent'?'tasks':n==='code'?'diff':'plan')`
                            // does in the prototype -- and, like it, opens the
                            // pane when the width leaves it no column to be
                            // shown in.
                            this.workspace = preset;
                            this.select_work_pane(preset.work_pane(), window);
                            cx.notify();
                        }))
                        .child(SharedString::from(preset.label()))
                        .test_support()
                }),
        )
        .test_support();

    // The prototype's session chip: a list glyph, the name, then a chevron --
    // and, because the prototype makes it a real `<button>`, the trigger of the
    // session dropdown. Each row now has behaviour behind it rather than a
    // notice: rename opens the name dialog, duplicate copies the conversation
    // into a new session, archive flips a flag the store keeps (never a
    // delete), and reveal hands the workspace to the desktop's file manager.
    // A pick also closes the menu it was picked from, the way a menu item does
    // everywhere else.
    let menu_owner = cx.weak_entity();
    let session_menu = Popover::new("session-menu")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(SessionChip::new(session_heading))
        .content(move |_state, _window, popover_cx| {
            // The menu is the popover this content builds, so a row press can
            // ask it to dismiss itself; the shell's own list is what the action
            // then edits or reloads.
            let menu = popover_cx.entity();
            let rename_owner = menu_owner.clone();
            let duplicate_owner = menu_owner.clone();
            let archive_owner = menu_owner.clone();
            let reveal_owner = menu_owner.clone();
            let archive_label = if open_archived {
                "Unarchive"
            } else {
                "Archive"
            };
            v_flex()
                .id("session-menu-panel")
                .test_support()
                .gap_1()
                .min_w(rems(13.))
                .child(
                    Button::new("session-menu-rename")
                        .icon(IconName::PencilLine)
                        .label("Rename")
                        .ghost()
                        .compact()
                        .on_click({
                            let menu = menu.clone();
                            move |_, window, cx| {
                                menu.update(cx, |state, cx| state.dismiss(window, cx));
                                let _ = rename_owner
                                    .update(cx, |app, cx| app.open_rename_dialog(window, cx));
                            }
                        }),
                )
                .child(
                    Button::new("session-menu-duplicate")
                        .icon(IconName::Copy)
                        .label("Duplicate")
                        .ghost()
                        .compact()
                        .on_click({
                            let menu = menu.clone();
                            move |_, window, cx| {
                                menu.update(cx, |state, cx| state.dismiss(window, cx));
                                let _ = duplicate_owner
                                    .update(cx, |app, cx| app.duplicate_open_session(cx));
                            }
                        }),
                )
                .child(
                    Button::new("session-menu-archive")
                        .icon(IconName::Archive)
                        .label(archive_label)
                        .ghost()
                        .compact()
                        .on_click({
                            let menu = menu.clone();
                            move |_, window, cx| {
                                menu.update(cx, |state, cx| state.dismiss(window, cx));
                                let _ = archive_owner.update(cx, |app, cx| {
                                    app.set_open_session_archived(!open_archived, cx)
                                });
                            }
                        }),
                )
                .child(
                    Button::new("session-menu-reveal")
                        .icon(IconName::FolderOpen)
                        .label("Reveal in filesystem")
                        .ghost()
                        .compact()
                        .on_click({
                            let menu = menu.clone();
                            move |_, window, cx| {
                                menu.update(cx, |state, cx| state.dismiss(window, cx));
                                let _ = reveal_owner.update(cx, |app, cx| app.reveal_workspace(cx));
                            }
                        }),
                )
        });

    // The prototype's `.cmd`: a bordered search field with the palette chord
    // in a `kbd` chip on the trailing edge. The chip only fits when the title
    // bar has room, so a compact bar keeps the icon alone.
    let command_hover = cx.theme().accent; // `.cmd:hover` uses `--hover`.
    let command_border = cx.theme().border;
    let command_border_hover = cx.theme().input;
    let mut command = h_flex()
        .id("open-command-palette")
        .test_support()
        .h(rems(1.75))
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .pl(rems(0.5625))
        .pr_2()
        .rounded(rems(0.5))
        .border_1()
        .border_color(command_border)
        .bg(cx.theme().popover)
        .text_color(cx.theme().muted_foreground)
        .hover(move |style| style.bg(command_hover).border_color(command_border_hover))
        .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
        .child(IconName::Search);
    if !compact {
        command = command
            .min_w(rems(9.625))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(rems(0.75))
                    .child(SharedString::from("Search or run a command")),
            )
            .child(kbd_chip(commands::hint("\u{2318}K", "Ctrl+K"), cx));
    }
    let command = tooltip_label(command, "Command palette (Ctrl+K)");

    let sidebar_toggle = tooltip_label(
        chrome_icon_button("toggle-sidebar", sidebar_icon, sidebar_open, cx),
        sidebar_action,
    )
    .aria_label(SharedString::from(sidebar_action))
    .on_click(cx.listener(|this, _, _, cx| {
        this.sidebar_open = !this.sidebar_open;
        cx.notify();
    }))
    .test_support();

    // `.tr` order in the prototype: the palette field, then theme, the work
    // pane toggle, and settings on the trailing edge.
    let controls = h_flex()
        .items_center()
        .gap(rems(0.375))
        .child(command)
        .child(
            tooltip_label(
                chrome_icon_button("toggle-theme", theme_icon, false, cx),
                theme_action,
            )
            .aria_label(SharedString::from(theme_action))
            .on_click(cx.listener(|_, _, window, cx| {
                theme::toggle(window, cx);
                let note = theme_toast(cx);
                push_toast(window, cx, note);
            }))
            .test_support(),
        )
        .child(
            tooltip_label(
                chrome_icon_button("toggle-work-pane", pane_icon, work_pane_open, cx),
                pane_action,
            )
            .aria_label(SharedString::from(pane_action))
            .on_click(cx.listener(|this, _, _, cx| {
                this.work_pane_open = !this.work_pane_open;
                cx.notify();
            }))
            .test_support(),
        )
        .child(
            tooltip_label(
                chrome_icon_button("open-settings", IconName::Settings, false, cx),
                "Settings (Ctrl+,)",
            )
            .aria_label(SharedString::from("Open settings"))
            .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)))
            .test_support(),
        );

    TitleBar::new().h(TITLE_BAR_HEIGHT).child(
        h_flex()
            .h_full()
            .w_full()
            .items_center()
            .child(
                // `.tl`: the sidebar column owns the left chrome. Native
                // window controls replace the prototype's traffic lights, so
                // the trailing space stays empty instead of shifting the
                // center column into the sidebar.
                h_flex()
                    .id("title-bar-left")
                    .test_support()
                    .when_else(
                        sidebar_float,
                        |this| this.h_full().flex_shrink_0(),
                        |this| this.w(columns.sidebar).h_full().flex_shrink_0(),
                    )
                    .items_center()
                    .gap(rems(0.625))
                    .px_3()
                    .child(sidebar_toggle)
                    .child(
                        div()
                            .size(rems(1.5))
                            .flex_shrink_0()
                            .rounded(rems(0.4375))
                            .bg(cx.theme().foreground)
                            .text_color(cx.theme().background)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(rems(0.75))
                            .font_bold()
                            .child(SharedString::from("T")),
                    ),
            )
            .child(
                // `.tc`: tabs and the session chip live in the main column,
                // aligned with the transcript below.
                h_flex()
                    .id("title-bar-center")
                    .test_support()
                    .min_w_0()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(tabs)
                    .child(session_menu),
            )
            .child(
                // `.tr`: the work-pane column's title-bar controls, including
                // the prototype's left hairline at the column boundary.
                h_flex()
                    .id("title-bar-right")
                    .test_support()
                    .when_else(
                        work_pane_float,
                        |this| this.h_full().flex_shrink_0(),
                        |this| this.w(columns.work_pane).h_full().flex_shrink_0(),
                    )
                    .items_center()
                    .justify_end()
                    .gap(rems(0.375))
                    .px(rems(0.625))
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .child(controls),
            ),
    )
}

/// Demo sessions for the design preview, shaped like the prototype's sidebar.
fn preview_sessions() -> Vec<RecentSession> {
    let now = session::now_unix();
    // Ages bucket into the prototype's 3 / 5 split: under a day reads as Today,
    // the rest fall inside the week.
    let rows = [
        (
            "7fbab10-2c41-4c9a-9f10-2222aaaa1111",
            "Desktop client design",
            60,
            12,
        ),
        (
            "3b78ba4-1d02-4a33-8b71-3333bbbb2222",
            "Review agent loop compaction",
            2_700,
            4,
        ),
        (
            "d464f22d-5e11-4c2f-9a08-4444cccc3333",
            "MCP reconnect bug",
            6_000,
            7,
        ),
        (
            "daf05fa-71b4-4d0e-8e55-5555dddd4444",
            "Token usage status bar",
            90_000,
            2,
        ),
        (
            "6278b7fa-3c92-4f18-9b27-6666eeee5555",
            "Remote MCP transport",
            100_000,
            5,
        ),
        (
            "306ea551-8a13-4b62-8f04-7777ffff6666",
            "Plugin compatibility menu",
            130_000,
            3,
        ),
        (
            "036e6015-a4d7-4e29-8c31-8888aaaa7777",
            "Compaction summary handoff",
            160_000,
            9,
        ),
        (
            "3f016556-2b8c-4f70-9d12-9999bbbb8888",
            "Work pane tab alignment",
            200_000,
            1,
        ),
    ];

    rows.iter()
        .map(|(id, title, age, messages)| RecentSession {
            id: (*id).to_string(),
            updated_at_unix: now - age,
            message_count: *messages,
            title: Some((*title).to_string()),
            name: None,
            // The prototype's `Remote MCP transport` row is the one that reads
            // as archived, so the preview shows the badge somewhere.
            archived: *title == "Remote MCP transport",
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
///
/// The names and owners are the prototype's own task table, so the Tasks pane,
/// its `Blocked by` card, and the plan pane's suggestion all render the design's
/// content rather than a second copy of the plan.
fn preview_tasks() -> Vec<tact_protocol::TaskSnapshot> {
    use tact_protocol::TaskStatusSnapshot;

    let rows = [
        (
            1,
            "Prototype shell",
            "agent",
            TaskStatusSnapshot::InProgress,
            Vec::new(),
        ),
        (
            2,
            "Theme registry",
            "agent",
            TaskStatusSnapshot::Completed,
            Vec::new(),
        ),
        (
            3,
            "Protocol event mapping",
            "agent",
            TaskStatusSnapshot::Pending,
            Vec::new(),
        ),
        (
            4,
            "Permission dialog review",
            "you",
            TaskStatusSnapshot::Pending,
            vec![3],
        ),
    ];

    rows.into_iter()
        .map(
            |(id, subject, owner, status, blocked_by)| tact_protocol::TaskSnapshot {
                id,
                subject: subject.to_string(),
                status,
                session_id: "preview".to_string(),
                owner: owner.to_string(),
                blocks: Vec::new(),
                blocked_by,
                created_at: None,
                started_at: None,
                completed_at: None,
            },
        )
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

/// `.row:hover` is the prototype's `--hover` while `.row.active` is its
/// `--accentTint`. They are different tokens, so a hovered row can never read
/// as the session the shell has open.
fn sidebar_row_fills(cx: &App) -> (gpui_kit::gpui::Hsla, gpui_kit::gpui::Hsla) {
    (cx.theme().accent, accent_tint(cx))
}

fn sidebar(
    state: &SessionState,
    recent: &[RecentSession],
    current: Option<&str>,
    search: &Entity<InputState>,
    width: Rems,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let session_line = session_activity_line(state);
    // Bound once: hover and highlight closures must not borrow the context.
    // `.row { border-radius: 7px }`: the prototype's own value, one pixel
    // above the theme's general 6 px.
    let radius = px(7.);
    let (hover_bg, current_bg) = sidebar_row_fills(cx);
    // `.row.active` rings itself with `inset 0 0 0 1px rgba(217,119,87,.08)`.
    let current_ring = cx.theme().primary.opacity(0.08);
    let muted = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let blue = cx.theme().info;
    let green = cx.theme().success;
    let mono = cx.theme().mono_font_family.clone();
    let now = session::now_unix();
    let query = search.read(cx).value().to_lowercase();
    let project = project_label(state);
    let branch = state.branch.as_deref();
    // A session's diffstat is not in the store, so only the open session can
    // carry one: it sums the changes this session's tools recorded, which is
    // what the Diff pane shows. A running turn reports itself instead.
    let (added, removed) = state.diff.iter().fold((0_u32, 0_u32), |totals, entry| {
        (
            totals.0 + entry.added.unwrap_or(0),
            totals.1 + entry.removed.unwrap_or(0),
        )
    });
    let open_diff = (!state.running && added + removed > 0).then_some((added, removed));

    // `.sideScroll` pads 4px and `.group:first-child` adds another 2px above
    // the first label; the horizontal and bottom insets already match.
    let mut list = v_flex().gap(rems(0.75)).px_2().pt(rems(0.375)).pb_3();
    for bucket in session_buckets(recent, &query, now) {
        let mut rows = v_flex().gap(rems(0.0625));
        for session in bucket.sessions {
            let is_current = current == Some(session.id.as_str());
            let session_id = session.id.clone();
            let diff = if is_current { open_diff } else { None };
            rows = rows.child(session_row(
                &session,
                is_current,
                &project,
                branch,
                diff,
                hover_bg,
                current_bg,
                current_ring,
                muted,
                primary,
                blue,
                green,
                mono.clone(),
                cx,
                move |this, cx| this.resume_session(session_id.clone(), cx),
            ));
        }

        list = list.child(
            v_flex()
                .gap_1()
                .child(group_label(&bucket.label, bucket.count, cx))
                .child(rows),
        );
    }

    if query.is_empty() && recent.is_empty() {
        list = list.child(
            v_flex()
                .id("sidebar-no-sessions")
                .test_support()
                .aria_label(SharedString::from("No sessions yet"))
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
            let mut rows = v_flex()
                .gap(rems(0.0625))
                .id("worktree-rows")
                .test_support();
            for worktree in &worktrees {
                rows = rows.child(worktree_row(worktree, radius, hover_bg, primary, cx));
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
            let mut rows = v_flex().gap(rems(0.0625));
            for task in &background {
                rows = rows.child(background_row(task, radius, hover_bg, primary, cx));
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
        .w(width)
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
    // `.new:hover` lifts the button to the prototype's `--hover`.
    let hover_bg = cx.theme().accent;
    let hover_border = crate::theme::ink3(cx);

    v_flex()
        .flex_shrink_0()
        .gap_2()
        .px(rems(0.625))
        .pt(rems(0.625))
        .pb(rems(0.5))
        .child(
            h_flex()
                .id("session-new")
                .test_support()
                .items_center()
                .h(rems(2.125))
                .gap_2()
                .border_1()
                .border_color(cx.theme().input)
                .bg(cx.theme().popover)
                .px(rems(0.625))
                .rounded(rems(0.5))
                .text_size(rems(0.78125))
                .font_semibold()
                .text_color(cx.theme().foreground)
                .hover(move |style| style.bg(hover_bg).border_color(hover_border))
                // `.new` is a `<button>` in the prototype, so the row is a tab
                // stop and takes the line 27 ring.
                .tab_index(0)
                .focus_visible({
                    let ring = focus_visible_ring(cx);
                    move |style| style.shadow(ring.clone())
                })
                .on_click(cx.listener(|this, _, _, cx| this.new_session(cx)))
                .child(IconName::Plus)
                .child(div().flex_1().child(SharedString::from("New session")))
                // `.new span:last-child`: the chord sits at the far edge in
                // the muted ink the prototype uses for hints.
                .child(
                    div()
                        .text_size(rems(0.6875))
                        .text_color(crate::theme::ink3(cx))
                        .child(SharedString::from(commands::hint("\u{2318}N", "Ctrl+N"))),
                ),
        )
        .child({
            let search = search.clone();
            // `.search:focus-within { border-color: var(--accent);
            // background: var(--surface); box-shadow: 0 0 0 2px
            // var(--accentTint) }`. Tracking the field's own handle makes the
            // wrapper's focused style mean "the field inside is focused",
            // which is what `:focus-within` asks for.
            let field_focus = search.focus_handle(cx);
            let accent = cx.theme().primary;
            let surface = cx.theme().popover;
            let ring = vec![
                gpui_kit::gpui::BoxShadow::new(px(0.), px(0.), accent_tint(cx))
                    .spread_radius(px(2.)),
            ];
            h_flex()
                .id("session-search")
                .test_support()
                .items_center()
                .h(rems(1.875))
                .gap(rems(0.4375))
                .border_1()
                .border_color(cx.theme().sidebar_border.opacity(0.0))
                .bg(cx.theme().muted)
                .px(rems(0.5))
                .rounded(rems(0.5))
                .text_color(crate::theme::ink3(cx))
                .track_focus(&field_focus)
                .focus(move |style| style.border_color(accent).bg(surface).shadow(ring.clone()))
                // The prototype's field is a native `<input>` with a
                // `:focus-within` ring, and a browser focuses it on the press
                // that lands anywhere in the label. GPUI does not, so the wrapper
                // has to hand the press to the input's focus handle — without
                // this the field is drawn and filterable but unreachable by
                // mouse.
                .on_mouse_down(MouseButton::Left, {
                    let search = search.clone();
                    move |_, window, cx| {
                        search.update(cx, |search, cx| {
                            search.focus_handle(cx).focus(window, cx);
                        });
                    }
                })
                .child(IconName::Search)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rems(0.75))
                        // `.search input{border:0;background:transparent}`:
                        // the well is the shell's own box, so the field inside
                        // it must not paint a second bordered one.
                        .child(Input::new(&search).appearance(false).bordered(false)),
                )
        })
        .id("sidebar-top")
        .test_support()
}

/// A `.badge` on a sidebar row: 17px tall, mono, and tinted by kind.
struct SessionBadge {
    text: String,
    fg: gpui_kit::gpui::Hsla,
    bg: gpui_kit::gpui::Hsla,
}

impl SessionBadge {
    fn render(&self, mono: gpui_kit::SharedString) -> impl IntoElement {
        div()
            .h(rems(1.0625))
            .flex_shrink_0()
            .flex()
            .items_center()
            .rounded(rems(0.3125))
            .bg(self.bg)
            .px(rems(0.3125))
            .font_family(mono)
            .text_size(rems(0.59375))
            .text_color(self.fg)
            .child(SharedString::from(self.text.clone()))
    }
}

/// The prototype's keyboard ring: line 27 declares
/// `button:focus-visible{outline:2px solid var(--accent);outline-offset:2px}`,
/// which every `.row`, `.tab`, `.icon`, `.cmd`, `.new`, `.mini`, `.wtab`,
/// `.toolbtn`, `.session` and `.send` inherits because the prototype draws each
/// of them as a `<button>`.
///
/// GPUI has no outline, so the ring is a `BoxShadow` with a 2 px spread: it
/// takes no part in layout, which is the outline property that matters here.
/// The prototype's `outline-offset` has no counterpart, so the ring hugs the box
/// -- from the inside, now -- instead of standing 2 px off it. The two
/// `:focus-within` rings use
/// `--accentTint` because the prototype draws those with `box-shadow`; this one
/// is the plain accent, as the prototype's outline is.
///
/// The ring is **inset**, and that is load-bearing rather than cosmetic: GPUI
/// paints a drop shadow as a *filled* rounded rect behind the element, so on any
/// control whose own background is transparent or translucent -- the session
/// rows (`.bg` only when current), the title-bar `.tab` chips -- that fill shows
/// straight through and the "ring" reads as a solid orange slab covering the
/// whole row (verified on screen, 2026-09-21). An inset shadow is drawn after
/// the background and before the children, so it stays a 2 px ring whatever the
/// background is.
pub(crate) fn focus_visible_ring(cx: &App) -> Vec<gpui_kit::gpui::BoxShadow> {
    vec![
        gpui_kit::gpui::BoxShadow::new(px(0.), px(0.), cx.theme().primary)
            .spread_radius(px(2.))
            .inset(),
    ]
}

/// One sidebar session row: status dot, title, and a metadata badge.
#[allow(clippy::too_many_arguments)]
fn session_row(
    session: &RecentSession,
    is_current: bool,
    project: &str,
    branch: Option<&str>,
    diff: Option<(u32, u32)>,
    hover_bg: gpui_kit::gpui::Hsla,
    current_bg: gpui_kit::gpui::Hsla,
    current_ring: gpui_kit::gpui::Hsla,
    muted: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    blue: gpui_kit::gpui::Hsla,
    green: gpui_kit::gpui::Hsla,
    mono: gpui_kit::SharedString,
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
    // `.running .dot` / `.review .dot` / `.done .dot`: the row's state picks the
    // dot's ink and the halo behind it. A session with no turns yet reads as the
    // neutral default rather than inventing a state.
    let (dot, halo) = if session.archived {
        // `.row` with no state class: an archived session is not running and has
        // no fresh diff, so its dot goes back to the prototype's neutral pair.
        (muted.opacity(0.5), muted.opacity(0.12))
    } else if is_current {
        (primary, primary.opacity(0.12))
    } else if session.message_count > 0 {
        (blue, blue.opacity(0.12))
    } else {
        (muted.opacity(0.5), muted.opacity(0.12))
    };
    // `.badge.run` / `.badge.diff` / plain `.badge`.
    let badge = if session.archived {
        // `.badge` with no state class -- the prototype's archived row. It wins
        // over the state badges because an archived session is not the live
        // state of anything, even when it happens to be the open row.
        Some(SessionBadge {
            text: "Archived".to_string(),
            fg: muted,
            bg: muted.opacity(0.10),
        })
    } else if is_current {
        Some(SessionBadge {
            text: "Running".to_string(),
            fg: cx.theme().accent_foreground,
            bg: accent_tint(cx),
        })
    } else if let Some((added, removed)) = diff {
        Some(SessionBadge {
            text: format!("+{added} \u{2212}{removed}"),
            fg: green,
            bg: green.opacity(0.12),
        })
    } else if session.message_count > 0 {
        Some(SessionBadge {
            text: "Review".to_string(),
            fg: muted,
            bg: muted.opacity(0.10),
        })
    } else {
        None
    };

    // The row's accessible name carries what it shows: the title, the metadata
    // line, and the state badge. A drawn row has no text of its own to fall back
    // on, so without this the sidebar is a wall of unlabelled buttons.
    let accessible = match badge.as_ref() {
        Some(badge) => format!("{label} · {meta} · {}", badge.text),
        None => format!("{label} · {meta}"),
    };

    h_flex()
        .id(SharedString::from(format!("session-row-{}", session.id)))
        .test_support()
        .aria_label(SharedString::from(accessible))
        // The row the shell has open, in the accessibility tree and in tests.
        // The prototype only paints `.row.active`, which no test can assert on.
        .aria_selected(is_current)
        .items_start()
        .min_h(rems(2.75))
        .gap(rems(0.4375))
        .px_2()
        .py(rems(0.4375))
        .rounded(rems(0.4375))
        .when(is_current, move |row| {
            row.bg(current_bg).border_1().border_color(current_ring)
        })
        .hover(move |style| style.bg(hover_bg))
        // The prototype's session rows are `<button>`s, so they are tab stops
        // with the line 27 ring. A drawn row is neither until it says so; the
        // element's own id gives the focus handle a stable identity across
        // frames.
        .tab_index(0)
        .focus_visible({
            let ring = focus_visible_ring(cx);
            move |style| style.shadow(ring.clone())
        })
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(
            // `.dot`: a 7px ink circle inside a 3px halo, so a running row
            // carries a visible ring rather than a hairline border.
            div()
                .mt(rems(0.3125))
                .size(rems(0.875))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(halo)
                .child(div().size(rems(0.4375)).rounded_full().bg(dot)),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.75))
                        .font_semibold()
                        .when(is_current, |title| {
                            title.text_color(cx.theme().accent_foreground)
                        })
                        .child(SharedString::from(label)),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(rems(0.3125))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(rems(0.65625))
                                .text_color(muted)
                                .child(SharedString::from(meta)),
                        )
                        .when_some(badge, |row, badge| row.child(badge.render(mono.clone()))),
                ),
        )
}

/// Sidebar row title: the name the user gave the session, then its opening
/// words, then the short id.
///
/// The prototype labels rows with human titles ("Desktop client design"); a
/// renamed session has to answer to the name the user typed, and a session whose
/// first message is missing or unparseable falls back to the id rather than
/// rendering an empty row.
fn session_row_title(session: &RecentSession) -> String {
    let label = session.name.as_deref().or(session.title.as_deref());
    match label {
        Some(title) if !title.is_empty() => title.to_string(),
        _ => session::short_id(&session.id).to_string(),
    }
}

/// The prototype's `kbd`: a 19px chip in 10.5px type.
fn kbd_chip(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .min_w(rems(1.1875))
        .h(rems(1.1875))
        .px(rems(0.25))
        .rounded(rems(0.3125))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .text_color(crate::theme::ink3(cx))
        .text_size(rems(0.65625))
        .child(SharedString::from(label))
}

/// The prototype's `.pfoot`: how to move, open, and dismiss from the palette.
fn palette_footer(muted: gpui_kit::gpui::Hsla) -> impl IntoElement {
    h_flex()
        .gap(rems(0.625))
        .text_size(rems(0.625))
        .text_color(muted)
        .child(SharedString::from("\u{2191}\u{2193} Navigate"))
        .child(SharedString::from("\u{21b5} Open"))
        .child(SharedString::from("esc Close"))
}

/// The session's name, from the store's opening message or the live transcript.
///
/// The store keeps no title column, so a session is named by the user's first
/// message. The session list already carries that text for every stored row;
/// a session this process just started is not in that list yet, so its live
/// transcript is the same words one step earlier. A session with no user turn
/// has no name at all, and the caller falls back to the workspace label.
fn session_name(
    recent: &[RecentSession],
    open: Option<&str>,
    rows: &[transcript::TranscriptRow],
) -> Option<String> {
    if let Some(id) = open
        && let Some(session) = recent.iter().find(|session| session.id == id)
    {
        // The user's own name wins over the opening words, exactly as it does in
        // the sidebar row this chip sits above.
        let named = session
            .name
            .as_deref()
            .and_then(tact_session::sessions::session_title);
        if named.is_some() {
            return named;
        }
        if let Some(title) = session.title.as_deref() {
            return tact_session::sessions::session_title(title);
        }
    }
    rows.iter().find_map(|row| match row {
        transcript::TranscriptRow::User { text, .. } => tact_session::sessions::session_title(text),
        _ => None,
    })
}

/// Display order for a request's actions.
///
/// The agent core hands over its choices as data (`Allow once`, `Deny`,
/// `Always allow this tool`); the prototype's `.actions` lead with the refusal
/// and then the grants. Only the permission trio is reordered — a question
/// keeps the order the agent gave — and the returned values stay the agent's
/// own indices, so the answer still names the choice that was clicked.
fn permission_action_order(options: &[String], permission: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..options.len()).collect();
    if permission {
        // `sort_by_key` is stable, so the grants keep their relative order.
        order.sort_by_key(|index| !options[*index].to_ascii_lowercase().contains("deny"));
    }
    order
}

/// One entry of `git worktree list`, trimmed to what the sidebar shows.
struct WorktreeRow {
    /// Branch name, or the directory's file name when detached.
    name: String,
    /// The worktree's own directory name, used as the description line.
    detail: String,
    /// The worktree's own directory, which a click re-roots the window at.
    path: PathBuf,
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
            path: path.to_path_buf(),
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
    primary: gpui_kit::gpui::Hsla,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let badge =
        worktree
            .is_current
            .then_some(("1 active", cx.theme().accent_foreground, accent_tint(cx)));
    let path = worktree.path.clone();
    sidebar_meta_row(
        SharedString::from(format!("worktree-row-{}", worktree.name)),
        SharedString::from(worktree.name.clone()),
        SharedString::from(worktree.detail.clone()),
        badge,
        // `.row`, not `.row.active`: the open worktree is told apart by its
        // `.badge.run` alone, so the dot stays neutral and the row keeps its
        // hover fill. The state itself still reaches the accessibility tree
        // through `selected`.
        false,
        false,
        Some(worktree.is_current),
        Some(Rc::new(
            move |this: &mut TactApp, cx: &mut Context<TactApp>| {
                this.switch_worktree(path.clone(), cx);
            },
        )),
        radius,
        hover_bg,
        primary,
        cx,
    )
}

/// A background row: the running command and its live badge.
fn background_row(
    task: &BackgroundRow,
    radius: gpui_kit::gpui::Pixels,
    hover_bg: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    sidebar_meta_row(
        SharedString::from(format!("background-row-{}", task.command)),
        SharedString::from(task.command.clone()),
        SharedString::from("Running"),
        Some(("Running", cx.theme().accent_foreground, accent_tint(cx))),
        true,
        false,
        None,
        None,
        radius,
        hover_bg,
        primary,
        cx,
    )
}

/// A sidebar row's click handler, or `None` for rows that are not controls.
type SidebarRowClick = Rc<dyn Fn(&mut TactApp, &mut Context<TactApp>)>;

/// The shared shape behind the worktree and background rows.
///
/// Sessions, worktrees, and background work all read as the prototype's
/// `.row`: a status dot, a title, a metadata line, and an optional badge. They
/// differ only in what those slots carry and whether the row is interactive,
/// so the geometry lives here once.
///
/// `selected` marks the row the window is currently scoped to. It is a row's
/// state, not its style, so it survives into the accessibility tree — the
/// prototype only paints `.row.active`, which nothing can assert on.
#[allow(clippy::too_many_arguments)]
fn sidebar_meta_row(
    id: SharedString,
    title: SharedString,
    meta: SharedString,
    badge: Option<(&'static str, gpui_kit::gpui::Hsla, gpui_kit::gpui::Hsla)>,
    active_dot: bool,
    highlighted: bool,
    selected: Option<bool>,
    on_click: Option<SidebarRowClick>,
    radius: gpui_kit::gpui::Pixels,
    hover_bg: gpui_kit::gpui::Hsla,
    primary: gpui_kit::gpui::Hsla,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let (dot, dot_bg) = if active_dot {
        // `.running .dot`: the ink over its own tint.
        (primary, accent_tint(cx))
    } else {
        // `.dot { background: var(--line2); box-shadow: 0 0 0 3px
        // var(--surface2) }`. The theme exposes those as `input` and `muted`;
        // ink2 is a text tier and would darken the neutral marker.
        (cx.theme().input, cx.theme().muted)
    };
    let mono = cx.theme().mono_font_family.clone();
    // `.row.active` / `.tree .row2.active` fill with `--accentTint`.
    let highlight_bg = accent_tint(cx);
    let accent_ink = cx.theme().accent_foreground;

    h_flex()
        .id(id)
        .test_support()
        .when_some(selected, |row, selected| row.aria_selected(selected))
        // Only the rows that do something carry a handler: a row that
        // highlights on hover and answers nothing is a dead control.
        .when_some(on_click, |row, on_click| {
            row.tab_index(0)
                .focus_visible({
                    let ring = focus_visible_ring(cx);
                    move |style| style.shadow(ring.clone())
                })
                .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        })
        .items_start()
        // `.row`: 44px minimum, 7px 8px padding, 7px gap.
        .min_h(rems(2.75))
        .gap(rems(0.4375))
        .px_2()
        .py(rems(0.4375))
        .rounded(radius)
        .when(highlighted, move |row| row.bg(highlight_bg))
        .hover(move |style| style.bg(hover_bg))
        .child(
            // `.dot`: a 7px ink circle inside a 3px halo, like the session
            // rows above it.
            div()
                .mt(rems(0.3125))
                .size(rems(0.875))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(dot_bg)
                .child(div().size(rems(0.4375)).rounded_full().bg(dot)),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.75))
                        .font_semibold()
                        .when(highlighted, move |title_div| {
                            title_div.text_color(accent_ink)
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
                                .text_size(rems(0.65625))
                                .text_color(crate::theme::ink3(cx))
                                .child(meta),
                        )
                        .when_some(badge, |row, (text, fg, bg)| {
                            row.child(
                                SessionBadge {
                                    text: text.to_string(),
                                    fg,
                                    bg,
                                }
                                .render(mono.clone()),
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
        .filter(|session| query.is_empty() || session.id.to_lowercase().contains(query))
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
        .pb(rems(0.3125))
        .text_size(rems(0.65625))
        .text_color(crate::theme::ink3(cx))
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
        .px(rems(0.625))
        .py(rems(0.5625))
        .child(
            div()
                .id("sidebar-avatar")
                .test_support()
                .size(rems(1.5))
                .flex_shrink_0()
                .rounded_full()
                .bg(cx.theme().primary)
                .text_size(rems(0.625))
                .font_bold()
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
                        .text_size(rems(0.71875))
                        .font_semibold()
                        .child(SharedString::from(project.to_string())),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.65625))
                        .text_color(crate::theme::ink3(cx))
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
    progress: f32,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    v_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .left(overlay_inset(SIDEBAR_OVERLAY_OFFSET, progress))
        .w(SIDEBAR_WIDTH)
        .border_r_1()
        .border_color(cx.theme().sidebar_border)
        .bg(cx.theme().sidebar)
        .shadow_xl()
        // The sidebar floats above the work pane's scrim -- `z-index:40`
        // against its `25` -- so it keeps the presses aimed at it instead of
        // letting the scrim close the work pane behind the user's back.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(sidebar(state, recent, current, search, SIDEBAR_WIDTH, cx))
        .id("sidebar-overlay")
        .test_support()
}

#[derive(Clone)]
struct TranscriptHeader {
    heading: SharedString,
    subtitle: Option<SharedString>,
    detail: transcript::TranscriptDetail,
}

/// Everything the transcript column needs to draw itself: its header text and
/// the column widths the prototype's `.thread` rule is measured against.
#[derive(Clone)]
struct TranscriptFrame {
    header: TranscriptHeader,
    columns: Columns,
}

/// A click listener stored in a virtual-list renderer, which only receives
/// `&mut App` after the owning view has finished its render pass.
type ShellClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

fn transcript(
    rows: &[transcript::TranscriptRow],
    state: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    session: &SessionState,
    attachments: &[Attachment],
    frame: TranscriptFrame,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let TranscriptFrame { header, columns } = frame;
    let TranscriptHeader {
        heading,
        subtitle,
        detail,
    } = header;
    let rows = rows.to_vec();
    let request = session.request.clone();
    let empty = rows.is_empty() && request.is_none();
    let subtitle =
        subtitle.unwrap_or_else(|| session_intro_subtitle(&project_label(session), empty));

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
    // The write row's diff badge carries its own handler: it opens the Diff
    // pane and stops the click from also toggling the tool body.
    let open_diff: transcript::OpenDiff = {
        let app = cx.entity().downgrade();
        Rc::new(move |cx: &mut App| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| {
                    app.set_work_pane(WorkPane::Diff);
                    app.work_pane_open = true;
                    cx.notify();
                });
            }
        })
    };

    // Answered approval rows render the same card the pending panel does, so
    // the row renderer borrows the shell's card instead of re-deriving it.
    let approval: transcript::ApprovalCard =
        Rc::new(|request, result, cx| answer_panel(request, result, cx).into_any_element());
    let cycle: ShellClick = Rc::new(cx.listener(|this, _, _, cx| this.cycle_detail(cx)));
    let focus_composer: ShellClick =
        Rc::new(cx.listener(|this, _, window, cx| this.focus_composer(window, cx)));
    let choose: Vec<ShellClick> = request
        .as_ref()
        .map(|request| {
            (0..request.options.len())
                .map(|index| {
                    Rc::new(cx.listener(move |this, _, _, cx| this.choose(index, cx))) as ShellClick
                })
                .collect()
        })
        .unwrap_or_default();
    let confirm: Option<ShellClick> = request
        .as_ref()
        .filter(|request| request.multi)
        .map(|_| Rc::new(cx.listener(|this, _, _, cx| this.confirm_multi(cx))) as ShellClick);
    let cancel: Option<ShellClick> = request
        .as_ref()
        .filter(|request| !is_permission_request(request))
        .map(|_| Rc::new(cx.listener(|this, _, _, cx| this.cancel_request(cx))) as ShellClick);

    let row_count = rows.len();
    let row_items = rows;
    let request_for_render = request;
    let choose_for_render = choose;
    let confirm_for_render = confirm;
    let cancel_for_render = cancel;
    let empty_focus = focus_composer;
    let cycle = cycle;
    let heading_for_render = heading;
    let subtitle_for_render = subtitle;

    let scroller = MessageScroller::new("transcript-list", state, move |index, window, cx| {
        if index == 0 {
            session_intro(
                &heading_for_render,
                &subtitle_for_render,
                detail,
                &cycle,
                cx,
            )
            .into_any_element()
        } else if empty {
            empty_transcript(&empty_focus, cx).into_any_element()
        } else if index <= row_count {
            transcript::render_row(
                &row_items[index - 1],
                index - 1,
                detail,
                transcript::RowActions {
                    toggle: &toggle,
                    open_diff: &open_diff,
                    approval: &approval,
                },
                window,
                cx,
            )
        } else if let Some(request) = &request_for_render {
            request_panel(
                request,
                &choose_for_render,
                confirm_for_render.as_ref(),
                cancel_for_render.as_ref(),
                cx,
            )
            .into_any_element()
        } else {
            div().into_any_element()
        }
    })
    .flex_1()
    .min_h_0()
    // The prototype's `.thread` is a 720px column with an 18px gap between
    // rows, so the scroller's built-in 12px row inset and 32px row gap are
    // overridden rather than left to stack on top of each row's own spacing.
    .with_row_style(StyleRefinement::default().px(px(0.)).pb(rems(1.125)))
    .with_bottom_fade(cx.theme().popover);

    let scroller = div()
        .flex_1()
        .min_h_0()
        .id("transcript-scroller")
        .test_support()
        .child(scroller);

    let body = v_flex()
        .w_full()
        .max_w(columns.thread)
        .mx_auto()
        .flex_1()
        .min_h_0()
        .child(scroller)
        .child(prompt_composer(&composer, session, attachments, cx));

    v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        // `.thread { width: min(720px, 100% - 48px) }`: the gutters are part of
        // the rule, so the body is capped at the column's width minus them.
        .px(columns.gutter)
        .child(transcript_toolbar(session, detail, cx))
        .child(body)
        .id("transcript")
        .test_support()
}

/// The shared branch / change / run state strip above the transcript.
/// The prototype's `.icon`: a 28 px square control with an 8 px radius whose
/// glyph is always 16 px and whose wash is `--hover`.
///
/// gpui-component's icon buttons are 32 px at their default size, so the shell
/// draws this one directly to keep the prototype's box. The toggling actions
/// reuse the hover wash, which is the only treatment `.icon` defines.
fn chrome_icon_button(id: &'static str, icon: IconName, active: bool, cx: &App) -> Stateful<Div> {
    let hover = cx.theme().accent;
    let ink = cx.theme().foreground;
    let muted = cx.theme().muted_foreground;
    h_flex()
        .id(id)
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(1.75))
        .rounded(rems(0.5))
        .when(active, |this| this.bg(hover).text_color(ink))
        .text_color(muted)
        .hover(move |style| style.bg(hover).text_color(ink))
        // `.icon` is a `<button>` in the prototype, so it takes the keyboard
        // ring there and the tab stop with it.
        .tab_index(0)
        .focus_visible({
            let ring = focus_visible_ring(cx);
            move |style| style.shadow(ring.clone())
        })
        .child(Icon::from(icon).with_size(px(16.)))
}

/// Attach the stock gpui-component tooltip to one of the shell's custom boxes.
///
/// The prototype's controls are plain `div`s, so the shell draws them directly
/// to keep the exact CSS box. `gpui-component`'s `Button` supplied the tooltip
/// before that; this keeps the affordance without giving up the measured size.
fn tooltip_label<E>(element: E, label: impl Into<SharedString>) -> E
where
    E: InteractiveElement + StatefulInteractiveElement + 'static,
{
    let label = label.into();
    element.tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
}

/// The prototype's `.btn` box: 28 px tall, 10 px inline padding, r8, 11.5 px
/// semibold text. `gpui-component`'s compact button is 32 px tall, so the
/// shell refines its `Button` rather than accepting the component default.
pub(crate) fn prototype_button(id: impl Into<SharedString>, primary: bool, cx: &App) -> Button {
    let button = Button::new(id.into())
        .h(rems(1.75))
        .px(rems(0.625))
        .rounded(px(8.))
        .text_size(rems(0.71875))
        .font_semibold();
    if primary {
        // `.btn.primary{...box-shadow:inset 0 1px 0 rgba(255,255,255,.2)}`. The
        // button's own `shadow(bool)` takes the name the styled setter wants, so
        // the shadow list is applied by path; the component refines the caller's
        // instance style on top of the variant's fill, so it survives.
        let inset = gpui_kit::gpui::BoxShadow::new(
            px(0.),
            px(1.),
            gpui_kit::Hsla::from(gpui_kit::rgba(0xffff_ffff)).opacity(0.2),
        )
        .inset();
        <Button as gpui_kit::Styled>::shadow(
            button
                .primary()
                .border_color(cx.theme().primary_active)
                .text_color(cx.theme().foreground),
            vec![inset],
        )
    } else {
        // `.btn.ghost{border:1px solid var(--line);background:var(--surface)}`
        // plus `.btn.ghost:hover{background:var(--hover);border-color:var(--line2)}`.
        // The `Ghost` variant can express neither half: its border is
        // `transparent` in every state (`button.rs:1032`), which erases the
        // resting outline the moment the pointer arrives, and its hover wash is
        // the theme accent rather than the prototype's `--hover`. `Custom`
        // holds one border colour across the states and takes an explicit hover
        // fill, so the outline survives the pointer. The hover step from
        // `--line` to `--line2` is the one part left unmatched, because a
        // button's own `hover` style cannot be extended from the call site.
        let variant = ButtonCustomVariant::new(cx)
            .color(cx.theme().border)
            .foreground(cx.theme().foreground)
            .hover(cx.theme().accent)
            .active(cx.theme().accent);
        button
            .custom(variant)
            .border_1()
            .bg(cx.theme().popover)
            .text_color(cx.theme().foreground)
    }
}

/// The prototype's `.icon` box: a 28 px square with an 8 px radius and the
/// standard ghost hover wash. Used by pane chrome as well as the title bar.
pub(crate) fn prototype_icon_button(
    id: impl Into<SharedString>,
    icon: IconName,
    cx: &App,
) -> Button {
    Button::new(id.into())
        .icon(icon)
        .with_size(px(28.))
        .ghost()
        .rounded(px(8.))
        .text_color(cx.theme().muted_foreground)
        .accessibility_label("Icon button")
}

/// The prototype's `.toolbtn`: a 26 px chip with an 8 px inline pad, a 6 px
/// gap, and 11.5 px text that washes with `--hover` on pointer-over.
///
/// gpui-component's buttons bottom out at 24 px (Small) and 32 px (Medium),
/// neither of which is the prototype's 26 px, so the shell draws this chip
/// directly. The numbers come from the prototype's CSS, not from the theme.
fn tool_button(
    id: &'static str,
    icon: IconName,
    label: Option<&'static str>,
    cx: &App,
) -> Stateful<Div> {
    let hover = cx.theme().accent;
    let ink = cx.theme().foreground;
    let mut chip = h_flex()
        .id(id)
        .flex_shrink_0()
        .items_center()
        .h(rems(1.625))
        .gap(rems(0.375))
        .px(rems(0.5))
        .rounded(rems(0.375))
        .text_size(rems(0.71875))
        .text_color(cx.theme().muted_foreground)
        .hover(move |style| style.bg(hover).text_color(ink))
        // `.toolbtn` is a `<button>` in the prototype; the drawn chip is a tab
        // stop with the same ring.
        .tab_index(0)
        .focus_visible({
            let ring = focus_visible_ring(cx);
            move |style| style.shadow(ring.clone())
        })
        .child(Icon::from(icon).with_size(px(16.)));
    if let Some(label) = label {
        chip = chip.child(SharedString::from(label));
    }
    chip
}

/// The prototype's `.cycle`: the same 26 px chip as `.toolbtn`, but bordered
/// and resting on `--surface2` because it is a control rather than a tool.
fn cycle_chip(id: &'static str, label: &'static str, cx: &App) -> Stateful<Div> {
    let hover = cx.theme().accent;
    let hover_border = cx.theme().input;
    h_flex()
        .id(id)
        .flex_shrink_0()
        .items_center()
        .h(rems(1.625))
        .gap(rems(0.375))
        .px(rems(0.5))
        .rounded(rems(0.375))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .text_size(rems(0.6875))
        .text_color(cx.theme().muted_foreground)
        .hover(move |style| style.bg(hover).border_color(hover_border))
        .child(Icon::from(IconName::RefreshCw).with_size(px(16.)))
        .child(SharedString::from(label))
}

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

    row.child(div().flex_1())
        .child(
            tooltip_label(
                tool_button(
                    "transcript-detail-cycle",
                    IconName::Eye,
                    Some(detail.label()),
                    cx,
                ),
                "Cycle transcript detail (Ctrl+O)",
            )
            .aria_label(SharedString::from(format!(
                "Transcript detail: {}",
                detail.label()
            )))
            .on_click(cx.listener(|this, _, _, cx| this.cycle_detail(cx)))
            .test_support(),
        )
        .child(
            // `.mainTop`'s second icon button: copy the transcript.
            tooltip_label(
                tool_button("transcript-copy", IconName::Copy, None, cx),
                "Copy transcript",
            )
            .aria_label(SharedString::from("Copy transcript"))
            .on_click(cx.listener(|this, _, _, cx| this.copy_transcript(cx)))
            .test_support(),
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
            cx.theme().accent_foreground,
            accent_tint(cx),
            cx.theme().transparent,
        )
    } else {
        (
            cx.theme().muted_foreground,
            cx.theme().muted,
            cx.theme().border,
        )
    };

    h_flex()
        .items_center()
        .h(rems(1.5))
        .gap(rems(0.3125))
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

fn session_intro_subtitle(project: &str, empty: bool) -> SharedString {
    if empty {
        SharedString::from(format!(
            "Start a task in {project}. The transcript stays primary; Plan, Diff, Tasks, and Files verify the work."
        ))
    } else {
        SharedString::from(format!(
            "Continue the task in {project}. Supporting detail stays in the work pane while the conversation remains readable."
        ))
    }
}

fn session_intro(
    heading: &SharedString,
    subtitle: &SharedString,
    detail: transcript::TranscriptDetail,
    cycle: &ShellClick,
    cx: &App,
) -> impl IntoElement {
    // The prototype's `.head`: heading and subtitle on the left, a detail
    // cycle chip on the right, over a hairline.
    h_flex()
        .w_full()
        .items_end()
        .justify_between()
        .gap_5()
        .pt(rems(1.375))
        .pb_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            v_flex()
                .min_w_0()
                .gap(rems(0.3125))
                .child(
                    div()
                        .text_size(rems(1.1875))
                        .font_semibold()
                        .child(heading.clone()),
                )
                .child(
                    div()
                        .max_w(rems(42.))
                        .text_size(rems(0.75))
                        .text_color(crate::theme::ink3(cx))
                        .child(SharedString::from(subtitle)),
                ),
        )
        .child(
            tooltip_label(
                cycle_chip("session-intro-detail-cycle", detail.label(), cx),
                "Cycle transcript detail (Ctrl+O)",
            )
            .aria_label(SharedString::from(format!(
                "Transcript detail: {}",
                detail.label()
            )))
            .on_click({
                let cycle = cycle.clone();
                move |event, window, cx| cycle(event, window, cx)
            })
            .test_support(),
        )
}

/// First-run placeholder shown before the transcript has any rows.
fn empty_transcript(focus: &ShellClick, cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
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
                .on_click({
                    let focus = focus.clone();
                    move |event, window, cx| focus(event, window, cx)
                }),
        )
        .id("transcript-empty")
        .test_support()
}

/// A blocking choice from the agent: permission prompts and `ask_user`.
///
/// The prototype's `.approval` card is the permission shape: a warning chip and
/// headline, the question, the command it wants to run, and its choices. A
/// question from `ask_user` reuses the same card with its own prompt.
fn request_panel(
    request: &Request,
    choose: &[ShellClick],
    confirm: Option<&ShellClick>,
    cancel: Option<&ShellClick>,
    cx: &App,
) -> impl IntoElement {
    let permission = is_permission_request(request);
    let mut actions = h_flex().w_full().flex_wrap().gap(rems(0.4375));
    for index in permission_action_order(&request.options, permission) {
        let option = &request.options[index];
        let selected = request.selected.contains(&index);
        // `.btn.primary` marks the lasting answer, which is the one that stops
        // the agent asking again.
        let lasting = permission && option.to_ascii_lowercase().contains("always");
        let choose = choose[index].clone();
        let button = prototype_button(
            SharedString::from(format!("request-option-{index}")),
            lasting,
            cx,
        )
        .label(option.clone())
        .toggled(selected)
        .on_click(move |event, window, cx| choose(event, window, cx));
        actions = actions.child(button);
    }

    if request.multi
        && let Some(confirm) = confirm
    {
        let confirm = confirm.clone();
        actions = actions.child(
            prototype_button("request-confirm", true, cx)
                .label("Confirm")
                .on_click(move |event, window, cx| confirm(event, window, cx)),
        );
    }
    // A permission prompt already offers `Deny` among its options; a question
    // has no way to decline without this.
    if !permission && let Some(cancel) = cancel {
        let cancel = cancel.clone();
        actions = actions.child(
            prototype_button("request-cancel", false, cx)
                .label("Cancel")
                .on_click(move |event, window, cx| cancel(event, window, cx)),
        );
    }

    approval_card(
        cx,
        vec![
            approval_details(request, cx).into_any_element(),
            actions.into_any_element(),
        ],
    )
}

/// An answered request: the same card with its decision in place of the actions.
///
/// This is the prototype's `.approval.done`, which keeps `Allowed once` where
/// the buttons were so the transcript still reads as a record. An answered card
/// is a transcript row, so it scrolls away with the turn it unblocked instead
/// of staying pinned below rows that arrived after it.
fn answer_panel(request: &Request, result: &str, cx: &App) -> impl IntoElement {
    approval_card(
        cx,
        vec![
            approval_details(request, cx).into_any_element(),
            h_flex()
                .items_center()
                .gap(rems(0.375))
                .text_size(rems(0.71875))
                .font_semibold()
                .text_color(cx.theme().success)
                .child(div().size(rems(0.8125)).child(IconName::Check))
                .child(SharedString::from(result.to_string()))
                .id("request-decision")
                .aria_label(SharedString::from(result.to_string()))
                .test_support()
                .into_any_element(),
        ],
    )
}

/// The `.approval` surface: 12px padding, a `--line2` border, and r10 corners.
fn approval_card(cx: &App, children: Vec<AnyElement>) -> impl IntoElement {
    v_flex()
        .w_full()
        .rounded(rems(0.625))
        .border_1()
        .border_color(cx.theme().input)
        .bg(cx.theme().muted)
        .p_3()
        .children(children)
        .id("request-panel")
        .test_support()
}

/// The card's headline, prompt, and the command it is asking about.
fn approval_details(request: &Request, cx: &App) -> impl IntoElement {
    let permission = is_permission_request(request);
    let command = permission_command(&request.prompt);
    let mut details = v_flex()
        .w_full()
        .child(request_headline(
            if permission {
                "Permission needed"
            } else {
                "Input needed"
            },
            cx,
        ))
        .child(
            div()
                .text_size(rems(0.75))
                // `.approval p { font-size: 12px; line-height: 1.5 }`.
                .line_height(relative(1.5))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(match command {
                    // The command moves into its own block, so the paragraph
                    // says what the card wants instead of repeating it.
                    Some(_) => "Tact wants to run this command:".to_string(),
                    None => request.prompt.clone(),
                })),
        );
    if let Some(command) = command {
        details = details.child(command_block(command, cx));
    }
    details
}

/// `.approval .headline`: a 22px warning chip beside a strong label.
fn request_headline(label: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .size(rems(1.375))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(rems(0.4375))
                .bg(accent_tint(cx))
                .text_color(cx.theme().accent_foreground)
                .child(IconName::TriangleAlert),
        )
        .child(
            div()
                .text_size(rems(0.78125))
                .font_semibold()
                .child(SharedString::from(label.to_string())),
        )
        .mb(rems(0.375))
}

/// `.approval code`: the command the agent is asking to run.
fn command_block(command: &str, cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .mt(rems(0.625))
        .mb(rems(0.6875))
        .rounded(rems(0.4375))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .px(rems(0.5625))
        .py(rems(0.5))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(rems(0.6875))
        .child(SharedString::from(command.to_string()))
}

/// Whether a request asks for permission rather than for an answer.
///
/// Both arrive as `RequestSelect`; the permission flow is the one whose options
/// are the `Allow once` / `Deny` / `Always allow this tool` trio.
fn is_permission_request(request: &Request) -> bool {
    request
        .options
        .iter()
        .any(|option| option.eq_ignore_ascii_case("allow once"))
}

/// The command a permission prompt is about, when the agent named one.
fn permission_command(prompt: &str) -> Option<&str> {
    prompt
        .strip_prefix("Run command: ")
        .map(str::trim)
        .filter(|command| !command.is_empty())
}

/// The prototype's `.mini`: a 25 px composer chip with a 7 px inline pad, a
/// 5 px gap, and 10.5 px text.
///
/// gpui-component's buttons bottom out at 24 px (Small, 12 px text) and its
/// default is 32 px (Medium, 16 px text), so the composer draws its own chip.
/// It has to be a named type because `Popover::trigger` requires `Selectable`,
/// which a bare div cannot implement.
#[derive(IntoElement)]
struct MiniTrigger {
    id: SharedString,
    leading: Option<IconName>,
    label: Option<SharedString>,
    trailing: Option<IconName>,
    tooltip: Option<SharedString>,
    strong: bool,
    selected: bool,
}

impl MiniTrigger {
    fn new(id: &'static str) -> Self {
        Self {
            id: SharedString::from(id),
            leading: None,
            label: None,
            trailing: None,
            tooltip: None,
            strong: false,
            selected: false,
        }
    }

    /// The glyph before the label, as the model chip's cycle mark is.
    fn leading(mut self, icon: IconName) -> Self {
        self.leading = Some(icon);
        self
    }

    fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The caret after the label, as the disclosure chips carry.
    fn trailing(mut self, icon: IconName) -> Self {
        self.trailing = Some(icon);
        self
    }

    /// The hover text the replaced `Button` used to own.
    fn tooltip(mut self, label: impl Into<SharedString>) -> Self {
        self.tooltip = Some(label.into());
        self
    }

    /// `.mini.strong`: the bordered chip the model selector wears.
    fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

impl Selectable for MiniTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for MiniTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            leading,
            label,
            trailing,
            tooltip,
            strong,
            selected,
        } = self;
        let chip = mini_chip(id, leading, label, trailing, strong, selected, cx);
        let chip = match tooltip {
            Some(label) => tooltip_label(chip, label),
            None => chip,
        };
        chip.test_support()
    }
}

/// The `.mini` box shared by the popover triggers and the plain `@` control.
#[allow(clippy::too_many_arguments)]
fn mini_chip(
    id: SharedString,
    leading: Option<IconName>,
    label: Option<SharedString>,
    trailing: Option<IconName>,
    strong: bool,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let hover = cx.theme().accent;
    let ink = cx.theme().foreground;
    let mut chip = h_flex()
        .id(id)
        .flex_shrink_0()
        .items_center()
        .h(rems(1.5625))
        .gap(rems(0.3125))
        .px(rems(0.4375))
        .rounded(rems(0.375))
        .text_size(rems(0.65625))
        .text_color(cx.theme().muted_foreground)
        .hover(move |style| style.bg(hover).text_color(ink))
        // `.mini` is a `<button>` in the prototype, so the drawn chip is a tab
        // stop, and the keyboard ring is what tells a keyboard user which chip
        // the arrow keys or Enter will act on.
        .tab_index(0)
        .focus_visible({
            let ring = focus_visible_ring(cx);
            move |style| style.shadow(ring.clone())
        });
    if strong {
        chip = chip
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted);
    }
    if selected {
        chip = chip.bg(hover).text_color(ink);
    }
    if let Some(leading) = leading {
        chip = chip.child(Icon::from(leading).with_size(px(16.)));
    }
    if let Some(label) = label {
        chip = chip.child(label);
    }
    if let Some(trailing) = trailing {
        chip = chip.child(Icon::from(trailing).with_size(px(16.)));
    }
    chip
}

/// The prototype's `.send`: a 28 px square primary action that dims to 38 %
/// while there is nothing to send and flips to `--ink` while a turn runs.
fn send_button(
    id: &'static str,
    icon: IconName,
    running: bool,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let idle = cx.theme().primary;
    let hover = cx.theme().primary_hover;
    let edge = cx.theme().primary_active;
    let ink = cx.theme().foreground;
    let run_bg = cx.theme().foreground;
    let page = cx.theme().background;
    h_flex()
        .id(id)
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(1.75))
        .rounded(rems(0.5))
        .border_1()
        // `.send{...box-shadow:inset 0 1px 0 rgba(255,255,255,.2)}`: the
        // hairline along the top edge is what makes the accent square read as
        // raised. `.send.run` swaps the fill and the border but does not reset
        // the shadow, so it stays on in both states. The white is the
        // prototype's own literal rather than a theme role.
        .shadow(vec![
            gpui_kit::gpui::BoxShadow::new(
                px(0.),
                px(1.),
                gpui_kit::Hsla::from(gpui_kit::rgba(0xffff_ffff)).opacity(0.2),
            )
            .inset(),
        ])
        .when(running, |this| {
            this.bg(run_bg).border_color(run_bg).text_color(page)
        })
        .when(!running, |this| {
            this.bg(idle)
                .border_color(edge)
                .text_color(ink)
                .hover(move |style| style.bg(hover))
        })
        .when(!enabled, |this| this.opacity(0.38))
        // `.send` is a `<button>` in the prototype. The ring replaces this
        // square's top hairline while it is focused, because a refined style
        // swaps the shadow list rather than appending to it.
        .tab_index(0)
        .focus_visible({
            let ring = focus_visible_ring(cx);
            move |style| style.shadow(ring.clone())
        })
        .child(Icon::from(icon).with_size(px(16.)))
}

fn prompt_composer(
    composer: &Entity<TextareaState>,
    session: &SessionState,
    attachments: &[Attachment],
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let running = session.running;
    let (primary_icon, primary_action) = if running {
        (IconName::SquareStop, "Stop the active turn")
    } else {
        (IconName::ArrowUp, "Send message")
    };
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
    let permission_modes = ["auto", "default", "plan"];
    let current_permission = session.permission_mode.clone();
    let muted_foreground = cx.theme().muted_foreground;
    let permission_label = permission_mode_label(&current_permission);

    let add_popover = Popover::new("composer-add-popover")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(
            MiniTrigger::new("composer-add")
                .leading(IconName::Plus)
                .tooltip("Add attachment, skill, connector, or plugin"),
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
            MiniTrigger::new("composer-model")
                .leading(IconName::RefreshCw)
                .label(current_model.clone())
                .trailing(IconName::ChevronDown)
                .strong()
                .tooltip("Model and reasoning controls"),
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

    let effort_owner = owner.clone();
    let effort_popover = Popover::new("composer-effort-popover")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(
            MiniTrigger::new("composer-effort")
                .label(effort_chip_label(current_effort.as_deref()))
                .tooltip("Reasoning effort"),
        )
        .content(move |_state, _window, _cx| {
            let mut panel = v_flex()
                .id("composer-effort-panel")
                .test_support()
                .gap_1()
                .min_w(rems(11.));
            for effort in [
                None,
                Some("low"),
                Some("medium"),
                Some("high"),
                Some("xhigh"),
                Some("max"),
            ] {
                let owner = effort_owner.clone();
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
            panel
        });

    let mention = tooltip_label(
        mini_chip(
            SharedString::from("composer-mention"),
            None,
            Some(SharedString::from("@")),
            None,
            false,
            false,
            cx,
        ),
        "Mention a file",
    )
    .on_click(cx.listener(|this, _, window, cx| this.insert_mention(window, cx)))
    .test_support();

    let permission_popover = Popover::new("composer-permission-popover")
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(
            MiniTrigger::new("composer-permission")
                .label(permission_label)
                .trailing(IconName::ChevronDown)
                .tooltip("Permission mode"),
        )
        .content(move |_state, _window, _cx| {
            let mut panel = v_flex()
                .id("composer-permission-panel")
                .test_support()
                .gap_1();
            for mode in permission_modes.iter().copied() {
                let owner = permission_owner.clone();
                let value = mode.to_string();
                panel = panel.child(
                    Button::new(SharedString::from(format!("composer-permission-{mode}")))
                        .label(permission_mode_label(mode))
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
    let mono_font = cx.theme().mono_font_family.clone();
    let muted_ink = crate::theme::ink3(cx);
    // `.ring`: a 24px circle whose centre is the percentage alone; the counts
    // stay in the popover and the accessible name.
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
                    // `.ring { width:24px; height:24px; }`. The component's
                    // compact button is 32 px tall, and it pads any button with
                    // children out to 34 wide, either of which grows the bar
                    // past the prototype's; a button with an icon would have
                    // taken `with_size` alone.
                    .with_size(px(24.))
                    .px(px(0.))
                    .child(
                        ProgressCircle::new("composer-usage-ring")
                            .value(percentage)
                            .small()
                            .size(rems(1.5))
                            .accessibility_label(label.clone())
                            .child(
                                div()
                                    .font_family(mono_font.clone())
                                    .text_size(rems(0.53125))
                                    .text_color(muted_ink)
                                    .child(SharedString::from(format!(
                                        "{}",
                                        percentage.round() as i64
                                    ))),
                            ),
                    ),
            )
            .content(move |_state, _window, _cx| usage_panel(usage_for_panel.clone()))
    });

    let mut controls = h_flex()
        .w_full()
        .items_center()
        .gap_1()
        .child(add_popover)
        .child(mention)
        .child(model_popover)
        .child(effort_popover)
        .child(permission_popover);

    if let Some(usage_ring) = usage_ring {
        controls = controls.child(usage_ring);
    }

    controls = controls.child(div().flex_1());

    // `.send`: a 28px icon-only square that is inert while there is nothing to
    // send, and swaps to the prototype's square stop glyph while a turn is in
    // flight.
    controls = controls.child(
        tooltip_label(
            send_button(
                "composer-primary",
                primary_icon,
                running,
                running || !draft.trim().is_empty(),
                cx,
            ),
            primary_action,
        )
        .aria_label(SharedString::from(primary_action))
        .on_click(cx.listener(|this, _, window, cx| {
            if this.state.running {
                this.stop(cx);
            } else {
                this.submit(window, cx);
            }
        }))
        .test_support(),
    );

    // `.composer { border: 1px solid var(--line2); border-radius: 12px;
    // background: var(--surface); box-shadow: 0 1px 2px rgba(20,20,19,.04),
    // 0 8px 24px rgba(20,20,19,.035) }`, and `.composer:focus-within {
    // border-color: var(--accent); box-shadow: 0 0 0 2px var(--accentTint) }`:
    // the ring replaces the resting shadow while the field inside has focus.
    let composer_focus = composer.focus_handle(cx);
    let accent = cx.theme().primary;
    let ring =
        vec![gpui_kit::gpui::BoxShadow::new(px(0.), px(0.), accent_tint(cx)).spread_radius(px(2.))];
    // `.composer` hard-codes its resting shadow as rgba(20,20,19,.04) and
    // rgba(20,20,19,.035) in both modes. `foreground` flips to near-white in
    // the dark theme, so it would paint a glow instead of a shadow.
    let shadow_ink = gpui_kit::Hsla::from(gpui_kit::rgba(0x1414_13ff));
    let mut body = v_flex()
        .id("composer-card")
        .test_support()
        .w_full()
        .rounded(rems(0.75))
        .border_1()
        .border_color(cx.theme().input)
        .bg(cx.theme().popover)
        .shadow(vec![
            gpui_kit::gpui::BoxShadow::new(px(0.), px(1.), shadow_ink.opacity(0.04))
                .blur_radius(px(2.)),
            gpui_kit::gpui::BoxShadow::new(px(0.), px(8.), shadow_ink.opacity(0.035))
                .blur_radius(px(24.)),
        ])
        .track_focus(&composer_focus)
        .focus(move |style| style.border_color(accent).shadow(ring.clone()));

    if !attachments.is_empty() {
        let mut chips = h_flex()
            .w_full()
            .flex_wrap()
            .gap(rems(0.375))
            .px(rems(0.5625))
            .pt(rems(0.5))
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
            // The prototype's `.chip button{color:var(--ink3)}`. The ghost
            // variant paints `secondary_foreground`, which is the prototype's
            // `--ink` -- two tiers too strong for a tertiary affordance.
            .text_color(crate::theme::ink3(cx))
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
                    .bg(cx.theme().muted)
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
            .px(rems(0.5625))
            .pt(rems(0.5))
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
            div()
                .id("prompt-composer-input")
                .test_support()
                .px(rems(0.6875))
                .pt(rems(0.625))
                .pb(rems(0.375))
                .child(
                    Textarea::new(composer)
                        .appearance(false)
                        .bordered(false)
                        .w_full()
                        .text_size(rems(0.8125))
                        .line_height(relative(1.5))
                        .accessibility_id("prompt-composer-field")
                        .aria_label("Message Tact"),
                ),
        )
        .child(controls.px(rems(0.4375)).pt(rems(0.3125)).pb(rems(0.4375)));

    v_flex()
        .w_full()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .px(rems(1.125))
        .pt(rems(0.625))
        .pb(rems(0.8125))
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

/// The prototype's theme toast: the palette that just became active.
fn theme_toast(cx: &App) -> Notification {
    let label = if Theme::global(cx).mode.is_dark() {
        "Dark theme"
    } else {
        "Light theme"
    };
    Notification::success(label)
}

/// Push a `.toast` through the window's root notification layer.
///
/// Handlers that hold a window (settings, the title bar) call this directly;
/// [`TactApp::toast`] queues the same message for the ones that do not.
fn push_toast(window: &mut Window, cx: &mut App, note: Notification) {
    let Some(root) = window.root::<Root>().flatten() else {
        return;
    };
    root.update(cx, |root, cx| root.push_notification(note, window, cx));
}

/// What a permission mode is called, in the prototype's words.
///
/// The composer chip and the status bar both name the mode, so they share this
/// mapping rather than each carrying their own list. A chip that does not know
/// the session's mode falls back to `Auto`, which is how the two used to
/// disagree about what the agent would ask before it acted.
///
/// Only the driver's three modes are named: `SetPermissionMode` reads anything
/// else as `Auto`, so an unknown mode is not quietly relabelled as `Ask`.
fn permission_mode_label(mode: &str) -> &'static str {
    match mode {
        "plan" => "Plan mode",
        "default" => "Ask permission",
        _ => "Auto approve",
    }
}

/// The effort chip's label, in the prototype's `High effort` shape.
fn effort_chip_label(effort: Option<&str>) -> String {
    let Some(effort) = effort.filter(|effort| !effort.is_empty()) else {
        return "Auto effort".to_string();
    };
    let mut characters = effort.chars();
    match characters.next() {
        Some(first) => format!("{}{} effort", first.to_uppercase(), characters.as_str()),
        None => "Auto effort".to_string(),
    }
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
    files: &mut FilesPane,
    diffs: &mut pane::DiffPane,
    width: Rems,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    v_flex()
        .flex_shrink_0()
        .w(width)
        .h_full()
        .border_l_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .child(pane::view(selected, state, files, diffs, cx))
        .id("work-pane")
        .test_support()
}

/// Work pane as a right drawer when the window is too narrow for three columns.
/// The prototype's `.scrim`, up while the work pane is out as a drawer.
///
/// `body.workOpen .scrim` covers everything but the drawer and
/// `q('#scrim').onclick=()=>work(false)`: a press anywhere behind the drawer
/// closes it instead of reaching the control under the pointer. Without it a
/// control that merely sits in the window answers a press the user aimed at the
/// drawer's surroundings -- the one difference `ElementSnapshot::visible()`,
/// an intersection with the viewport, cannot tell apart from reachability.
fn work_pane_scrim(cx: &mut Context<TactApp>) -> impl IntoElement {
    // `.scrim{background:rgba(20,20,19,.14)}` -- the same wash in both themes.
    let wash = gpui_kit::Hsla::from(gpui_kit::rgba(0x1414_1324));
    div()
        .absolute()
        .inset_0()
        .bg(wash)
        .id("work-pane-scrim")
        .test_support()
        // Claim the press before it reaches the hitboxes under the scrim: the
        // sidebar's rows and the transcript both sit at the same point, and
        // without this a click would land on the scrim *and* on whichever
        // control the drawer happens to be covering.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, _, cx| {
            this.work_pane_open = false;
            cx.notify();
        }))
}

fn work_pane_drawer(
    selected: WorkPane,
    state: &SessionState,
    files: &mut FilesPane,
    diffs: &mut pane::DiffPane,
    progress: f32,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    v_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .right(overlay_inset(WORK_PANE_DRAWER_OFFSET, progress))
        .w(WORK_PANE_WIDTH)
        .border_l_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .shadow_xl()
        // The drawer is above the scrim (`z-index:30` against `25`), so it
        // claims the press on its way out: a click on the pane's own tabs must
        // switch the pane, not read as a click on the scrim behind it.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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

    let permission = permission_mode_label(state.permission_mode.as_str());

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
        .gap(rems(0.625))
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().tab_bar)
        .px(rems(0.6875))
        .text_size(rems(0.65625))
        .text_color(crate::theme::ink3(cx))
        .child(status_item(
            "status-project",
            Some(IconName::Box),
            project,
            // `.status strong` is `--ink2`, not the transcript's full ink.
            cx.theme().muted_foreground,
            cx,
        ));

    if let Some(branch) = state.branch.as_deref() {
        bar = bar.child(status_item(
            "status-branch",
            Some(IconName::GitBranch),
            branch.to_string(),
            crate::theme::ink3(cx),
            cx,
        ));
    }

    bar = bar.child(
        h_flex()
            .items_center()
            .gap(rems(0.3125))
            .flex_shrink_0()
            .text_color(crate::theme::ink3(cx))
            .child(
                div()
                    .text_color(cx.theme().accent_foreground)
                    .child("\u{25cf}"),
            )
            .child(SharedString::from(permission))
            .id("status-permission")
            .aria_label(SharedString::from(permission))
            .test_support(),
    );

    if added > 0 || removed > 0 {
        bar = bar.child(
            h_flex()
                .items_center()
                .gap(rems(0.3125))
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
                )
                .id("status-diff")
                .aria_label(SharedString::from(format!("+{added} \u{2212}{removed}")))
                .test_support(),
        );
    }

    if let Some(turns) = state.turns {
        let label = match turns {
            (taken, Some(max)) => format!("turn {taken}/{max}"),
            (taken, None) => format!("turn {taken}"),
        };
        bar = bar.child(status_item(
            "status-turns",
            None,
            label,
            crate::theme::ink3(cx),
            cx,
        ));
    }

    bar = bar.child(div().flex_1());

    if let Some(context) = context {
        bar = bar.child(status_item(
            "status-context",
            None,
            context,
            crate::theme::ink3(cx),
            cx,
        ));
    }

    // The prototype closes the bar with the account balance; the provider only
    // reports one for accounts that track it, so the chip is conditional.
    if let Some(balance) = balance_label(state) {
        bar = bar.child(status_item(
            "status-balance",
            None,
            balance,
            crate::theme::ink3(cx),
            cx,
        ));
    }

    if running > 0 {
        bar = bar.child(status_item(
            "status-running",
            None,
            format!("{running} running"),
            cx.theme().accent_foreground,
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
///
/// The `id` is what makes a chip observable: without it the segment is not in
/// the walk's registry, and the footer contract (which chip reads which piece
/// of session state) can only be asserted by reading the source.
fn status_item(
    id: &'static str,
    icon: Option<IconName>,
    label: String,
    color: gpui_kit::Hsla,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap(rems(0.3125))
        .flex_shrink_0()
        .text_color(color)
        .font_family(cx.theme().font_family.clone())
        .children(icon.map(|icon| Icon::from(icon).with_size(px(12.))))
        .child(SharedString::from(label.clone()))
        .id(id)
        .aria_label(SharedString::from(label))
        .test_support()
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
                            push_toast(window, cx, Notification::success("Light theme"));
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
                            push_toast(window, cx, Notification::success("Dark theme"));
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
        SessionBucket, Workspace, background_rows, balance_label, next_session_index,
        permission_action_order, permission_mode_label, previous_session_index, session_buckets,
        session_name, session_row_title, worktree_rows,
    };
    use crate::pane::WorkPane;
    use crate::session::SessionHandle;
    use crate::{RecentSession, session::SessionState, transcript};
    use tact_session::{HistoryBlock, HistoryMessage, HistoryRole};

    /// The Stop half of the composer's primary control reaches the session.
    ///
    /// A running turn is only reachable from a live agent session, so this test
    /// builds the handle's sender half itself and reads back what the press
    /// dispatched. The receiver is the proof: the square stop glyph renders
    /// identically whether or not the press reached the agent.
    #[gpui_kit::test]
    fn the_stop_control_cancels_the_running_turn(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};
        use tact_protocol::UserCommand;

        cx.update(gpui_kit::init);

        let (commands, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionHandle::new("test-session".to_string(), commands);
        let handle = cx.open_window(size(px(1440.), px(900.)), move |window, cx| {
            let shell = cx.new(|cx| super::TactApp::with_workspace(window, cx, None));
            shell.update(cx, |app, _| {
                app.session = Some(session);
                app.state.running = true;
            });
            Root::new(shell, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("composer-primary", cx);
            window.render_frame(cx);
        })
        .unwrap();

        assert!(
            matches!(dispatched.try_recv(), Ok(UserCommand::Cancel)),
            "the primary control cancels the turn it is drawn for"
        );
        assert!(
            dispatched.try_recv().is_err(),
            "and cancelling dispatches nothing else"
        );
    }

    /// The Send half hands the draft to the session, not only to the transcript.
    ///
    /// The integration suite presses this control and reads the row it appends,
    /// but an offline shell appends that row whether or not a session is
    /// attached, so it cannot tell a queued command from a painted one. The
    /// channel here is what separates the two.
    #[gpui_kit::test]
    fn the_send_control_hands_the_draft_to_the_session(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};
        use tact_protocol::UserCommand;

        cx.update(gpui_kit::init);

        let (commands, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionHandle::new("test-session".to_string(), commands);
        let handle = cx.open_window(size(px(1440.), px(900.)), move |window, cx| {
            let shell = cx.new(|cx| super::TactApp::with_workspace(window, cx, None));
            shell.update(cx, |app, cx| {
                app.session = Some(session);
                app.composer
                    .update(cx, |state, cx| state.set_value("Ship the diff", window, cx));
            });
            Root::new(shell, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("composer-primary", cx);
            window.render_frame(cx);
        })
        .unwrap();

        assert!(
            matches!(dispatched.try_recv(), Ok(UserCommand::SubmitTask(task)) if task == "Ship the diff"),
            "an idle shell hands the draft to the session"
        );
    }

    /// The palette-only commands reach the session as protocol commands.
    ///
    /// The integration suite proves each of these three rows answers, but an
    /// offline shell answers with a notice; the mapping from the row to the
    /// command the driver understands is only visible with a session attached.
    /// They are the three the palette reaches and no chord does.
    #[gpui_kit::test]
    fn the_palette_only_commands_map_onto_their_protocol_commands(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::{px, size};
        use tact_protocol::UserCommand;

        cx.update(gpui_kit::init);

        let (commands, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionHandle::new("test-session".to_string(), commands);
        let _window = cx.open_window(size(px(1440.), px(900.)), move |window, cx| {
            let shell = cx.new(|cx| super::TactApp::with_workspace(window, cx, None));
            shell.update(cx, |app, _| app.session = Some(session));
            shell.update(cx, |app, cx| {
                for command in [
                    super::PaletteCommand::CompactSession,
                    super::PaletteCommand::SessionStats,
                    super::PaletteCommand::McpServers,
                ] {
                    app.run_palette_command(command, window, cx);
                }
            });
            Root::new(shell, window, cx)
        });

        assert!(
            matches!(dispatched.try_recv(), Ok(UserCommand::Compact)),
            "the compact row asks the session to compact"
        );
        assert!(
            matches!(dispatched.try_recv(), Ok(UserCommand::QueryStats)),
            "the stats row asks the session for its token stats"
        );
        assert!(
            matches!(dispatched.try_recv(), Ok(UserCommand::McpList)),
            "the MCP row asks the session for its servers"
        );
        assert!(
            dispatched.try_recv().is_err(),
            "and the three rows dispatch exactly one command each"
        );
    }

    /// An answered approval card stops being the tail of the transcript.
    ///
    /// The card used to live in a slot *beside* the list, so once it was
    /// answered it stayed the last thing on screen for the rest of the session
    /// while the turn it unblocked appended rows above it. It is a conversation
    /// row now: the answer files it in the slot it was asked in, and whatever
    /// the turn writes next lands below it.
    ///
    /// Both halves matter to the regression. Order alone would not catch a card
    /// that is copied into the list *and* left in the slot, so the item count is
    /// pinned too: a pending request is still one item past the rows, an
    /// answered one is only its own row.
    #[gpui_kit::test]
    fn an_answered_approval_keeps_its_slot_instead_of_the_tail(cx: &mut gpui_kit::TestAppContext) {
        use crate::session::Request;
        use crate::transcript::TranscriptRow;
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::{px, size};

        cx.update(gpui_kit::init);

        let mut shell = None;
        let handle = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                let app = cx.new(|cx| super::TactApp::with_workspace(window, cx, None));
                app.update(cx, |app, _| {
                    app.state.request = Some(Request {
                        id: 7,
                        prompt: "Run command: cargo check -p tact-gui".to_string(),
                        options: vec![
                            "Allow once".to_string(),
                            "Deny".to_string(),
                            "Always allow this tool".to_string(),
                        ],
                        multi: false,
                        selected: Vec::new(),
                    });
                });
                shell = Some(app.clone());
                Root::new(app, window, cx)
            })
            .into();
        let shell = shell.expect("the shell is created with its window");

        cx.update_window(handle, |_, _, cx| {
            shell.update(cx, |app, cx| {
                // The seeded request is one item past the single header.
                assert_eq!(
                    app.transcript_item_count(),
                    2,
                    "a pending request is a card at the tail, extra to the rows"
                );

                // Row 0 of the seeded trio is `Allow once`.
                app.choose(0, cx);
                assert!(app.state.request.is_none(), "the pending slot is cleared");

                let rows = app.conversation.rows();
                assert_eq!(rows.len(), 1, "the answer files the card as one row");
                assert!(
                    matches!(
                        rows[0],
                        TranscriptRow::Approval { ref result, .. } if result == "Allow once"
                    ),
                    "the row is the answered card, phrased by its own label: {rows:?}"
                );
                assert_eq!(
                    app.transcript_item_count(),
                    2,
                    "and the answered card is the row, not a card beside it"
                );

                // The turn resumes: a later row must land *below* the record.
                app.push_system_row("the turn moved on".to_string(), cx);
                let rows = app.conversation.rows();
                assert!(
                    matches!(rows[0], TranscriptRow::Approval { .. }),
                    "the record keeps the slot it was asked in: {rows:?}"
                );
                assert!(
                    matches!(rows[1], TranscriptRow::System { .. }),
                    "the next row follows it instead of being pushed above it: {rows:?}"
                );
                assert_eq!(
                    app.transcript_item_count(),
                    3,
                    "two rows beside the header, and no card left at the tail"
                );
            });
        })
        .unwrap();
    }

    /// A permission request that arrives mid-turn lands wholly on screen.
    ///
    /// The pending card is the one item past the rows, so it is also the item a
    /// follow-tail scroller has to reach. `sync_transcript_count` resets the
    /// list when the extra item appears and then asks for the end; the question
    /// this pins is whether the reset leaves the new row's *height* unknown
    /// long enough for the offset to fall short, which would put the allow and
    /// deny buttons under the composer.
    #[gpui_kit::test]
    fn an_arriving_request_lands_wholly_inside_the_transcript(cx: &mut gpui_kit::TestAppContext) {
        use crate::session::{Change, Request};
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};

        cx.update(gpui_kit::init);

        let mut shell = None;
        let handle = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                let app = cx.new(|cx| super::TactApp::with_workspace(window, cx, None));
                shell = Some(app.clone());
                Root::new(app, window, cx)
            })
            .into();
        let shell = shell.expect("the shell is created with its window");

        cx.update_window(handle, |_, window, cx| {
            shell.update(cx, |app, cx| {
                // A turn long enough that the transcript overflows its viewport.
                for index in 0..40 {
                    app.push_system_row(format!("step {index}"), cx);
                }

                // The live arrival path: the session sets the request, then the
                // shell records the change it returned (`Change::None`).
                app.state.request = Some(Request {
                    id: 9,
                    prompt: "Run command: cargo check -p tact-gui".to_string(),
                    options: vec![
                        "Allow once".to_string(),
                        "Deny".to_string(),
                        "Always allow this tool".to_string(),
                    ],
                    multi: false,
                    selected: Vec::new(),
                });
                app.record_change(Change::None, cx);
            });

            window.render_frame(cx);
            window.render_frame(cx);

            let list = window.find("transcript").bounds();
            let card = window.find("request-panel").bounds();
            assert!(
                card.bottom() <= list.bottom(),
                "the whole permission card is on screen: card {card:?} inside list {list:?}"
            );
        })
        .unwrap();
    }

    /// The work pane's five prototype-only actions are not dead controls.
    ///
    /// The broad click walk can only show that a control renders and survives a
    /// press, which is exactly the shape these five used to hide in: they
    /// carried no handler at all. Each now answers with the reason it cannot
    /// act yet, and every reason is its own, so pressing the whole set leaves
    /// five distinct rows rather than four silences and one notice.
    #[gpui_kit::test]
    fn the_pane_actions_v1_does_not_back_each_answer_a_press(cx: &mut gpui_kit::TestAppContext) {
        use crate::pane::WorkPane;
        use crate::transcript::TranscriptRow;
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{App, px, size};
        use std::collections::HashSet;
        use std::path::PathBuf;

        cx.update(gpui_kit::init);

        let mut shell = None;
        let handle = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                // The Files pane only draws its head against a real workspace,
                // so the shell opens on the repository the manifest lives in.
                let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .nth(2)
                    .map(PathBuf::from);
                let app = cx.new(|cx| super::TactApp::with_workspace(window, cx, workdir));
                shell = Some(app.clone());
                Root::new(app, window, cx)
            })
            .into();
        let shell = shell.expect("the shell is created with its window");

        let row_text = |shell: &gpui_kit::Entity<super::TactApp>, id: &str, cx: &mut App| {
            shell.update(cx, |app, _| match app.conversation.rows().last() {
                Some(TranscriptRow::System { text }) => text.clone(),
                other => panic!("{id} appends a system row, not {other:?}"),
            })
        };

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);

            let mut seen: Vec<String> = Vec::new();
            for (pane, id) in [
                (WorkPane::Plan, "work-pane-plan-refresh"),
                (WorkPane::Diff, "work-pane-diff-comment"),
                (WorkPane::Tasks, "work-pane-tasks-new"),
                (WorkPane::Files, "work-pane-files-add"),
            ] {
                shell.update(cx, |app, cx| {
                    app.set_work_pane(pane);
                    app.work_pane_open = true;
                    cx.notify();
                });
                window.render_frame(cx);
                assert!(
                    window.try_find(id).is_some(),
                    "{id} is rendered on its own pane"
                );
                window.click(id, cx);
                window.render_frame(cx);
                seen.push(row_text(&shell, id, cx));
            }

            // `Open in editor` is the footer every pane shares.
            assert!(
                window.try_find("work-pane-open-editor").is_some(),
                "the footer renders its action"
            );
            window.click("work-pane-open-editor", cx);
            window.render_frame(cx);
            seen.push(row_text(&shell, "work-pane-open-editor", cx));

            let unique: HashSet<&String> = seen.iter().collect();
            assert_eq!(
                unique.len(),
                5,
                "each prototype-only action answers with its own reason: {seen:?}"
            );
            for text in &seen {
                assert!(
                    text.contains("not available yet"),
                    "the row states the limit instead of a silent press: {text}"
                );
            }
        })
        .unwrap();
    }

    /// Escape stops at the layer that owns it.
    ///
    /// `escape` is StopTask for the shell, but an open palette or dialog takes
    /// the press first, and the turn behind it must survive. The last phase is
    /// the control: with nothing over the shell the same press is the turn's
    /// own stop, so the two earlier silences are not a dead binding.
    #[gpui_kit::test]
    fn escape_stops_at_the_layer_that_owns_it(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};
        use std::time::Duration;
        use tact_protocol::UserCommand;

        cx.update(gpui_kit::init);
        cx.update(crate::commands_init);

        let (commands, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionHandle::new("test-session".to_string(), commands);
        let handle = cx
            .open_window(size(px(1440.), px(900.)), move |window, cx| {
                let shell = cx.new(|cx| super::TactApp::with_workspace(window, cx, None));
                shell.update(cx, |app, _| {
                    app.session = Some(session);
                    app.state.running = true;
                });
                Root::new(shell, window, cx)
            })
            .into();

        // The palette is the top layer while it is up.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("open-command-palette", cx);
            // Dialog entrance is a 250 ms wall-clock animation.
            std::thread::sleep(Duration::from_millis(400));
            window.render_frame(cx);
            assert!(
                window.try_find("command").is_some(),
                "the palette opens over the shell"
            );
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("command").is_none(),
                "escape reaches the focused palette"
            );
        })
        .unwrap();
        assert!(
            dispatched.try_recv().is_err(),
            "the palette consumed escape instead of passing it on to StopTask"
        );

        // So is the settings dialog.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("open-settings", cx);
            std::thread::sleep(Duration::from_millis(400));
            window.render_frame(cx);
            assert!(
                window.try_find("settings-theme-light").is_some(),
                "the dialog opens over the shell"
            );
            window.press("escape", cx);
            std::thread::sleep(Duration::from_millis(400));
            window.render_frame(cx);
            assert!(
                window.try_find("settings-theme-light").is_none(),
                "escape reaches the focused dialog"
            );
        })
        .unwrap();
        assert!(
            dispatched.try_recv().is_err(),
            "the dialog consumed escape instead of passing it on to StopTask"
        );

        // With nothing over it, the press belongs to the shell.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
            window.render_frame(cx);
        })
        .unwrap();
        assert!(
            matches!(dispatched.try_recv(), Ok(UserCommand::Cancel)),
            "bare escape is the running turn's own stop"
        );
    }

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
            rows.iter()
                .all(|row| !row.name.is_empty() && !row.detail.is_empty()),
            "every row carries a branch and a directory"
        );
        // A click re-roots the window at the row's own directory, so the path
        // has to travel with the row the sidebar renders.
        assert!(
            rows.iter().all(|row| row.path.is_dir()),
            "every row carries a directory that exists"
        );
        assert!(
            rows.iter().any(|row| row.path.is_absolute()),
            "the worktree list reports absolute paths"
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
        assert!(
            balance_label(&state).is_none(),
            "no account update, no chip"
        );

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

    #[test]
    fn a_session_is_named_by_its_opening_message() {
        let rows = vec![
            transcript::TranscriptRow::Assistant {
                markdown: "here is the plan".to_string(),
                streaming: false,
                sent_at: 0,
                model: None,
            },
            transcript::TranscriptRow::User {
                text: "  Build   the shell\nfirst  ".to_string(),
                sent_at: 0,
            },
        ];
        let stored = vec![RecentSession {
            id: "session-a".to_string(),
            updated_at_unix: 0,
            message_count: 2,
            title: Some("Desktop client design".to_string()),
            name: None,
            archived: false,
        }];

        // The stored title wins, so the chip and the sidebar row agree about
        // the session even while its transcript is still streaming.
        assert_eq!(
            session_name(&stored, Some("session-a"), &rows).as_deref(),
            Some("Desktop client design")
        );
        // Without a stored row the live transcript names it, collapsing
        // whitespace the way the sidebar title does.
        assert_eq!(
            session_name(&[], None, &rows).as_deref(),
            Some("Build the shell first")
        );
        // A session whose only turn so far is the agent's has no name.
        assert_eq!(session_name(&[], None, &rows[..1]), None);
    }

    #[test]
    fn a_permission_prompt_leads_with_the_refusal() {
        let options: Vec<String> = ["Allow once", "Deny", "Always allow this tool"]
            .map(str::to_string)
            .to_vec();
        // The agent's own order is `Allow once`, `Deny`, then the lasting
        // grant; the prototype prints the refusal first, then the grants.
        assert_eq!(permission_action_order(&options, true), vec![1, 0, 2]);
        // A question keeps the order the agent gave it, and a permission
        // prompt with no refusal has nothing to move.
        assert_eq!(permission_action_order(&options, false), vec![0, 1, 2]);
        let without_refusal: Vec<String> = ["Yes", "No"].map(str::to_string).to_vec();
        assert_eq!(permission_action_order(&without_refusal, true), vec![0, 1]);
    }

    #[test]
    fn each_top_tab_selects_the_pane_the_prototype_pairs_with_it() {
        // The prototype's `.tab` handler: chat→plan, agent→tasks, code→diff.
        assert_eq!(Workspace::Chat.work_pane(), WorkPane::Plan);
        assert_eq!(Workspace::Agent.work_pane(), WorkPane::Tasks);
        assert_eq!(Workspace::Code.work_pane(), WorkPane::Diff);
    }

    #[test]
    fn a_session_row_prefers_its_title_and_falls_back_to_the_short_id() {
        let mut titled = session("daf05fa-71b4-4d0e-8e55-5555dddd4444", 10, 1_700_000_000);
        assert_eq!(session_row_title(&titled), "daf05fa");

        titled.title = Some("Desktop client design".to_string());
        assert_eq!(session_row_title(&titled), "Desktop client design");

        // A store row whose opening message was blank keeps the id, not a gap.
        titled.title = Some(String::new());
        assert_eq!(session_row_title(&titled), "daf05fa");
    }

    fn session(id: &str, age_seconds: i64, now: i64) -> RecentSession {
        RecentSession {
            id: id.to_string(),
            updated_at_unix: now - age_seconds,
            message_count: 1,
            title: None,
            name: None,
            archived: false,
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

    /// The prototype uses one spelling per permission mode, and both the
    /// composer chip and the status bar read it from here.
    #[test]
    fn permission_modes_have_one_spelling() {
        assert_eq!(permission_mode_label("default"), "Ask permission");
        assert_eq!(permission_mode_label("plan"), "Plan mode");
        assert_eq!(permission_mode_label("auto"), "Auto approve");
        assert_eq!(
            permission_mode_label("ask"),
            "Auto approve",
            "the driver reads an unknown mode as auto, so the label must not claim otherwise"
        );
    }

    /// A workspace a connected shell can start a real runtime in.
    ///
    /// `SessionRuntime::start` opens `<workdir>/.tact/tact.db`, so a writable
    /// directory is all the fixture has to supply -- the store creates its own
    /// parent directory. The provider is process-global and has to be installed
    /// before the session thread reaches for it; it is deliberately keyless, so
    /// the agent reports `AgentUpdate::Error` on the stream instead of taking
    /// the panic path in a background thread.
    /// Removes a fixture workspace once the test that made it ends.
    ///
    /// The runtime holds the sqlite file open on its own thread, so the guard
    /// only unlinks the directory -- whatever the session thread is still
    /// finishing keeps working against the open descriptor.
    struct WorkspaceCleanup(std::path::PathBuf);

    impl Drop for WorkspaceCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn connected_workdir() -> std::path::PathBuf {
        static INSTALLING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _installing = INSTALLING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        tact_session::test_support::install_test_config();

        let dir = std::env::temp_dir().join(format!(
            "tact-gui-session-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the fixture makes its own workspace");
        dir
    }

    /// The New-session control starts a real `tact-session` runtime.
    ///
    /// Every other contract in this suite drives an offline shell, whose
    /// `new_session` returns before it reaches `session::start`. The connected
    /// branch is the one that touches the store, and "start a session" is what
    /// the prototype's `New session` row claims, so it is pinned here rather
    /// than read: the press has to hand the window a handle, and that session
    /// has to exist in the workspace's own database afterwards.
    #[gpui_kit::test]
    fn the_new_session_control_starts_a_runtime_and_writes_its_row(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};

        cx.update(gpui_kit::init);
        let workdir = connected_workdir();
        let _cleanup = WorkspaceCleanup(workdir.clone());

        let mut shell = None;
        let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
            let app = cx.new(|cx| {
                let mut app = super::TactApp::with_workspace(window, cx, Some(workdir.clone()));
                app.offline = false;
                app
            });
            shell = Some(app.clone());
            Root::new(app, window, cx)
        });
        let shell = shell.expect("the window built a shell");

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("session-new", cx);
            window.render_frame(cx);

            let started = shell
                .update(cx, |app, _| {
                    app.session
                        .as_ref()
                        .map(|session| session.session_id().to_string())
                })
                .expect("the press adopted a session");
            assert!(
                crate::session::recent(&workdir)
                    .iter()
                    .any(|row| row.id == started),
                "the runtime wrote its own row: {started} is not in the workspace store"
            );
            assert_eq!(
                shell.update(cx, |app, _| app.open_session_id().map(str::to_string)),
                Some(started),
                "the sidebar marks the session the runtime started"
            );
        })
        .unwrap();
    }

    /// Cycling sessions resumes the runtime the row names.
    ///
    /// An offline shell only moves the row its sidebar marks open, which the
    /// integration suite already covers. The connected branch goes through
    /// `resume_session` into `session::resume` and so into the store, so this
    /// test starts two real sessions through the control and then cycles
    /// between them, asserting the adopted id is the other row's id rather than
    /// only the highlighted row.
    #[gpui_kit::test]
    fn cycling_sessions_resumes_the_runtime_the_row_names(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};

        cx.update(gpui_kit::init);
        cx.update(crate::commands_init);
        let workdir = connected_workdir();
        let _cleanup = WorkspaceCleanup(workdir.clone());

        let mut shell = None;
        let handle = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                let app = cx.new(|cx| {
                    let mut app = super::TactApp::with_workspace(window, cx, Some(workdir.clone()));
                    app.offline = false;
                    app
                });
                shell = Some(app.clone());
                Root::new(app, window, cx)
            })
            .into();
        let shell = shell.expect("the window built a shell");

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            // Two presses leave two rows in the workspace's own store.
            window.click("session-new", cx);
            window.render_frame(cx);
            window.click("session-new", cx);
            window.render_frame(cx);

            let (rows, started) = shell.update(cx, |app, _| {
                (
                    app.recent
                        .iter()
                        .map(|row| row.id.clone())
                        .collect::<Vec<_>>(),
                    app.open_session_id().map(str::to_string),
                )
            });
            assert_eq!(rows.len(), 2, "two presses leave two stored sessions");
            let started = started.expect("the second press adopted a session");

            window.press("ctrl-tab", cx);
            window.render_frame(cx);

            let resumed = shell
                .update(cx, |app, _| {
                    app.session
                        .as_ref()
                        .map(|session| session.session_id().to_string())
                })
                .expect("cycling leaves a session attached");
            let expected = rows
                .iter()
                .find(|id| id.as_str() != started)
                .expect("a second row exists to cycle to")
                .clone();
            assert_eq!(
                resumed, expected,
                "Primary+Tab resumes the row it moved to, not only the highlighted one"
            );
            assert_eq!(
                shell.update(cx, |app, _| app.recent.len()),
                2,
                "resuming reuses the row instead of starting another session"
            );
        })
        .unwrap();
    }

    /// Switching sessions redraws the transcript the store still holds.
    ///
    /// The reported bug: clicking a session in the sidebar adopted its runtime
    /// but handed the window a blank page, so the conversation vanished until
    /// the next turn. The store keeps the canonical blocks, so a resume
    /// redraws them — in stored order, which is what keeps `thinking -> tool ->
    /// answer` reading the way the turn produced it.
    #[gpui_kit::test]
    fn switching_sessions_redraws_the_stored_transcript(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{px, size};

        cx.update(gpui_kit::init);
        cx.update(crate::commands_init);
        let workdir = connected_workdir();
        let _cleanup = WorkspaceCleanup(workdir.clone());

        let session_id = "7d0e2f14-8a51-4b7a-9c2e-5f6a1b2c3d4e".to_string();
        tact_session::test_support::seed_session_history(
            &workdir,
            &session_id,
            &[
                HistoryMessage {
                    role: HistoryRole::User,
                    blocks: vec![HistoryBlock::Text("check the build".into())],
                },
                HistoryMessage {
                    role: HistoryRole::Assistant,
                    blocks: vec![
                        HistoryBlock::Thinking("weighing".into()),
                        HistoryBlock::ToolUse {
                            id: "tool_1".into(),
                            name: "bash".into(),
                            detail: "cargo test".into(),
                        },
                    ],
                },
                HistoryMessage {
                    role: HistoryRole::User,
                    blocks: vec![HistoryBlock::ToolResult {
                        tool_use_id: "tool_1".into(),
                        output: "ok\n".into(),
                    }],
                },
                HistoryMessage {
                    role: HistoryRole::Assistant,
                    blocks: vec![HistoryBlock::Text("it passes".into())],
                },
            ],
        );

        let mut shell = None;
        let handle = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                let app = cx.new(|cx| {
                    let mut app = super::TactApp::with_workspace(window, cx, Some(workdir.clone()));
                    app.offline = false;
                    app
                });
                shell = Some(app.clone());
                Root::new(app, window, cx)
            })
            .into();
        let shell = shell.expect("the window built a shell");

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            shell.update(cx, |app, cx| app.resume_session(session_id.clone(), cx));
            window.render_frame(cx);

            let rows = shell.update(cx, |app, _| app.conversation.rows().to_vec());
            assert_eq!(rows.len(), 5, "the stored turns plus the resume notice: {rows:?}");
            assert!(matches!(
                &rows[0],
                transcript::TranscriptRow::User { text, .. } if text == "check the build"
            ));
            assert!(matches!(
                &rows[1],
                transcript::TranscriptRow::Thinking { text, .. } if text == "weighing"
            ));
            assert!(matches!(
                &rows[2],
                transcript::TranscriptRow::Tool { display_name, detail, output, .. }
                    if display_name == "bash" && detail == "cargo test" && output.trim() == "ok"
            ));
            assert!(matches!(
                &rows[3],
                transcript::TranscriptRow::Assistant { markdown, .. } if markdown == "it passes"
            ));
            assert!(
                matches!(&rows[4], transcript::TranscriptRow::System { text } if text.contains("Resumed")),
                "the resume notice still lands after the redrawn turns"
            );
            // The scroller has to be told the transcript grew, or the redrawn
            // rows render past the end of the virtual list.
            assert_eq!(
                shell.update(cx, |app, _| app.transcript_item_count()),
                6,
                "header + five rows"
            );
        })
        .unwrap();
    }
}
