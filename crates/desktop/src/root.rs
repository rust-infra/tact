use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AppContext as _, ClickEvent, Context, Entity,
    FollowMode, InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment, ListOffset,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    PathBuilder, Render, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Task, WeakEntity, Window, canvas, div, list, point, prelude::FluentBuilder as _, px,
};
use gpui_base::FocusTrapElement as _;
use gpui_component::popover::Popover;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, IconNamed as _, Sizable as _, StyledExt as _, TitleBar,
    h_flex,
    input::Input,
    text::{TextView, TextViewStyle},
    v_flex,
};
use tact_catalog::{
    ArgumentValue, BuiltinCommand, ChoiceSource, CommandArguments, CommandDescriptor, CommandId,
    SettingId,
};
use tact_desktop_core::{
    Appearance, ArchiveFilter, CommandAvailability, CommandForm, DesktopController, DesktopEffect,
    DesktopEvent, DesktopIntent, DesktopSnapshot, DesktopTheme, ExtensionsTab, GateCard, GateKind,
    GateResponse, GateSelection, GateState, InspectorTab, PermissionModeView, ProjectId, ReaderRef,
    RowBody, RowId, RowStatus, SectionMask, SessionId, SessionPhase, SettingValue, Submission,
    SurfaceSnapshot, ToolGroupItem, ToolRow, TrajectoryRecordId, TrajectoryView, TranscriptRow,
    TranscriptView, is_enabled,
};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::i18n::{self, TextKey};
use crate::ui::command_search::{
    CommandSearch, CommandSearchEvent, CommandSearchItem, CommandSearchLabels,
};
use crate::ui::diff_table::{
    DiffCell, DiffChangeKind, DiffColumn, DiffProposalAction, DiffProposalState, DiffRow,
    DiffTable, DiffTableEvent, DiffTableLabels,
};
use crate::ui::file_links;
use crate::ui::filter_table::{
    ActivationSource, FilterCell, FilterColumn, FilterDefinition, FilterRow, FilterTable,
    FilterTableEvent,
};
use crate::ui::leaf::{
    AttachmentItem, AttachmentStrip, ContextCard, ContextCardEvent, ContextMeter, ContextUsage,
    LoadingState, Orbs, Suggestion, Suggestions, SuggestionsEvent, Toggle, ToggleEvent,
    ToggleShape, ToolChip, ToolStatus, is_image_path,
};
use crate::ui::plan_card::{PlanCard, PlanCardLabels, PlanState, PlanStep, PlanStepStatus};
use crate::ui::process::{
    ProcessGroup, ProcessRowRole, answers_only_visible, build_process_layout, is_process_row,
    plan_is_tool_execution, process_summary_text, row_starts_at_message_column,
};
use crate::ui::prompt_bar::{
    ProgressState, PromptBar, PromptBarEvent, PromptBarLabels, PromptCommand, PromptMention,
    PromptModel, PromptSubmit,
};
use crate::ui::question_flow::{ChoiceOption, Question, QuestionFlow, QuestionFlowEvent};
use crate::ui::reader_components::{
    Artifact, ArtifactKind, ArtifactPanel, ArtifactPanelEvent, ArtifactView, CodeBlock, CodeDiff,
    DiffFile, InsightCard, MessageQueue, MessageQueueLabels, QueueEvent, QueuedMessage,
    SearchResult, SearchResults, SearchResultsEvent,
};
use crate::ui::row_scroll::RowScroll;
use crate::ui::selection_actions::{SelectionAction, SelectionActions, SelectionActionsEvent};
use crate::ui::shell::{RunTab, TrajectoryDetailTab};
use crate::ui::sidebar_nav::{
    SidebarNav, SidebarNavEvent, SidebarNavItem, SidebarNavLabels, SidebarNavPresentation,
    SidebarSection,
};
use crate::ui::status::{
    background_status_label, goal_phase_label, goal_status_label, localized_wait_reason,
    mcp_status_label, run_phase_label, subagent_status_label,
};
use crate::ui::sticky::GroupStatus;
use crate::ui::surface;
use crate::ui::trajectory as trajectory_page;
use crate::ui::transcript_components::{
    StreamedContent, StreamingText, Thinking, ThinkingEvent, ThinkingLabels, ToolCall,
    ToolCallEvent, ToolCallLabels, ToolGroup, ToolGroupEvent,
};
use crate::ui::{ansi, diff_review, mermaid, web_search};
use crate::{keymap, theme};

const TRANSCRIPT_SESSION_CACHE: usize = 6;

/// A permission gate repeats the command it asks about. Past this many lines
/// the preview scrolls instead of growing, so the decision stays in view.
const GATE_COMMAND_PREVIEW_LINES: usize = 8;
const GATE_COMMAND_MAX_HEIGHT: f32 = 168.;

/// One row of a picker menu, in pixels.
const MODEL_PICKER_ROW_HEIGHT: f32 = 30.;
/// Rows a model picker shows before it scrolls.
const MODEL_PICKER_VISIBLE_ROWS: usize = 8;
/// Informational notifications disappear on their own; errors stay until the
/// user dismisses them.
const TOAST_HIDE_DELAY: Duration = Duration::from_secs(3);
#[cfg(not(test))]
const SCROLL_COALESCE_INTERVAL: Duration = Duration::from_millis(24);
#[cfg(test)]
const SCROLL_COALESCE_INTERVAL: Duration = Duration::ZERO;

#[derive(Clone)]
struct Toast {
    title: SharedString,
    body: SharedString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DividerSide {
    Rail,
    Inspector,
}

#[derive(Clone, Copy, Debug)]
struct DividerDrag {
    side: DividerSide,
    pointer_x: f32,
    width: f32,
}

#[derive(Clone)]
struct ReaderState {
    title: SharedString,
    markdown: SharedString,
    /// Shared with the chat transcript's Markdown selection surface so reader
    /// text follows the same click, double-click and drag semantics.
    selection: Option<Entity<SelectionActions>>,
    /// Owns the reader's scroll viewport so wheel input invalidates only the
    /// Markdown body instead of rebuilding the whole Desktop shell.
    markdown_view: Option<Entity<ReaderMarkdownView>>,
    typed: Option<ReaderKind>,
    /// Which transcript row (or static surface) produced this reader. The
    /// shell uses it to mount the typed surfaces — ANSI, search results,
    /// artifacts and multi-file review — instead of raw Markdown.
    reference: Option<ReaderRef>,
    artifact_view: ArtifactView,
    /// Built on first render for a multi-file diff and reused across frames.
    diff_table: Option<Entity<DiffTable>>,
    /// One decision per file of a multi-file diff, in patch order.
    decisions: Vec<DiffProposalState>,
    /// Bumped when a decision changes; `diff_applied` records the epoch the
    /// table rows were last built from.
    diff_epoch: u64,
    diff_applied: u64,
}

impl ReaderState {
    fn new(title: impl Into<SharedString>, markdown: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            markdown: markdown.into(),
            selection: None,
            markdown_view: None,
            typed: None,
            reference: None,
            artifact_view: ArtifactView::Preview,
            diff_table: None,
            decisions: Vec::new(),
            diff_epoch: 0,
            diff_applied: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReaderKind {
    SystemPrompt,
    Stats,
    History,
    TasksDag,
    Balance,
}

#[derive(Clone)]
struct FormState {
    title: SharedString,
    action: FormAction,
    field: Option<String>,
    required: bool,
    choice: Option<Vec<(String, String)>>,
}

#[derive(Clone, Debug)]
struct SlashSuggestion {
    title: String,
    detail: String,
    insertion: String,
    has_children: bool,
}

#[derive(Clone, Debug)]
struct SlashOverlayState {
    suggestions: Vec<SlashSuggestion>,
    active: usize,
}

#[derive(Clone)]
enum FormAction {
    Command(CommandId),
    RenameSession(SessionId),
    CreateGoal,
    SteerGoal(String),
    AcceptGoal(String),
    McpAddUrl(String),
    McpAddTransport(String),
    McpAddChoose(String),
    McpAddCommand(String),
    McpPrompt { server: String, name: String },
    RemoveProject(ProjectId),
    ConfirmCancelRun,
}

#[derive(Clone)]
enum Overlay {
    CommandCenter,
    Settings,
    Form(FormState),
    Reader(ReaderState),
    Subagent(String),
    ModelPicker(SettingId),
    Extensions(ExtensionsTab),
    Hooks,
    SessionManager,
}

/// The shell's view of the voice worker.
#[derive(Clone, Copy, Debug, Default)]
struct VoiceUi {
    enabled: bool,
    missing_api_key: bool,
    listening: bool,
    transcribing: bool,
}

struct TranscriptRowView {
    row: Arc<TranscriptRow>,
    snapshot: Arc<DesktopSnapshot>,
    root: WeakEntity<DesktopRoot>,
    assistant_selection: Option<Entity<SelectionActions>>,
    /// Inline expansion state for a thinking row.
    thinking_open: bool,
    /// The first action of a pending gate tracks this handle, so Review can
    /// move focus onto the primary allow option without selecting it.
    gate_focus: gpui::FocusHandle,
}

/// One assistant row's selectable prose.
///
/// The row Arc is the cache key: the reducer replaces it only when the
/// message changed, so a frame with no edits skips stripping Mermaid blocks
/// and re-laying-out every answer in the transcript.
struct AssistantSelection {
    row: Arc<TranscriptRow>,
    selection: Entity<SelectionActions>,
    language: tact_catalog::CatalogLocale,
    appearance: Appearance,
}

impl Render for TranscriptRowView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        transcript_row_element(
            &self.row,
            &self.snapshot,
            self.root.clone(),
            self.assistant_selection.clone(),
            false,
            Some(&self.gate_focus),
            self.thinking_open,
        )
    }
}

struct ReaderMarkdownView {
    selection: Entity<SelectionActions>,
    scroll: ScrollHandle,
    pending_scroll: Rc<Cell<f32>>,
    scroll_scheduled: Rc<Cell<bool>>,
    scroll_task: Rc<RefCell<Option<Task<()>>>>,
}

impl Render for ReaderMarkdownView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        div()
            .id("reader-scroll-content")
            .debug_selector(|| "reader-scroll-content".into())
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(self.selection.clone())
            .child(coalesced_wheel_canvas(
                self.pending_scroll.clone(),
                self.scroll_scheduled.clone(),
                self.scroll_task.clone(),
                move |delta, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        let offset = view.scroll.offset();
                        let minimum = -view.scroll.max_offset().y;
                        let next = (offset.y + px(delta)).clamp(minimum, px(0.));
                        view.scroll.set_offset(point(px(0.), next));
                        cx.notify();
                    });
                },
            ))
    }
}

fn coalesced_wheel_canvas(
    pending: Rc<Cell<f32>>,
    scheduled: Rc<Cell<bool>>,
    task: Rc<RefCell<Option<Task<()>>>>,
    apply: impl Fn(f32, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let apply = Rc::new(apply);
    canvas(
        // The hitbox lets the capture handler ask gpui's cached hit test
        // whether the wheel belongs to this surface. A popup painted above
        // occludes it, so the popup's own scroll area keeps the event.
        |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            let apply = apply.clone();
            let pending = pending.clone();
            let scheduled = scheduled.clone();
            let task = task.clone();
            let hitbox_id = hitbox.id;
            window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                if !(phase.capture() && hitbox_id.should_handle_scroll(window)) {
                    return;
                }
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                pending.set(pending.get() + delta);
                if !scheduled.replace(true) {
                    let apply = apply.clone();
                    let pending = pending.clone();
                    let scheduled = scheduled.clone();
                    *task.borrow_mut() = Some(cx.spawn(async move |cx| {
                        loop {
                            cx.background_executor()
                                .timer(SCROLL_COALESCE_INTERVAL)
                                .await;
                            let delta = pending.replace(0.);
                            if delta != 0. {
                                cx.update(|cx| apply(delta, cx));
                            }
                            if pending.get() != 0. {
                                continue;
                            }
                            scheduled.set(false);
                            if pending.get() == 0. {
                                break;
                            }
                            if scheduled.replace(true) {
                                break;
                            }
                        }
                    }));
                }
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .inset_0()
}

struct TranscriptListState {
    list_state: ListState,
}

impl TranscriptListState {
    fn new(item_count: usize) -> Self {
        let list_state = ListState::new(item_count, ListAlignment::Top, px(400.));
        list_state.set_follow_mode(FollowMode::Tail);
        Self { list_state }
    }

    fn item_count(&self) -> usize {
        self.list_state.item_count()
    }

    fn is_scrolled_up(&self) -> bool {
        self.list_state.max_offset_for_scrollbar().y > px(0.)
            && !self.list_state.is_following_tail()
            && !self.list_state.is_scrolled_to_end().unwrap_or(false)
    }

    fn reset(&mut self, item_count: usize, cx: &mut Context<Self>) {
        self.list_state.reset(item_count);
        self.list_state.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    fn splice(
        &mut self,
        old_range: std::ops::Range<usize>,
        count: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if old_range.start > old_range.end || old_range.end > self.list_state.item_count() {
            return false;
        }
        let neighbor = old_range.start.checked_sub(1);
        self.list_state.splice(old_range, count);
        if let Some(last) = self.list_state.item_count().checked_sub(1) {
            self.list_state.remeasure_items(last..last + 1);
            if let Some(neighbor) = neighbor.filter(|neighbor| *neighbor != last) {
                self.list_state.remeasure_items(neighbor..neighbor + 1);
            }
        }
        cx.notify();
        true
    }

    fn append(&mut self, count: usize, cx: &mut Context<Self>) -> bool {
        let item_count = self.list_state.item_count();
        self.splice(item_count..item_count, count, cx)
    }

    fn remeasure_items(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> bool {
        if range.start > range.end || range.end > self.list_state.item_count() {
            return false;
        }
        self.list_state.remeasure_items(range);
        cx.notify();
        true
    }

    fn scroll_to_item(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        if index >= self.list_state.item_count() {
            return false;
        }
        self.list_state.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: px(0.),
        });
        cx.notify();
        true
    }

    fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list_state.set_follow_mode(FollowMode::Tail);
        self.list_state.scroll_to_end();
        cx.notify();
    }
}

struct TranscriptScrollerView {
    scroller: Entity<TranscriptListState>,
    rows: Arc<Vec<Entity<TranscriptRowView>>>,
    root: WeakEntity<DesktopRoot>,
    session_id: SessionId,
    language: tact_catalog::CatalogLocale,
    tail_following: bool,
    new_activity: bool,
    wheel_enabled: bool,
    answers_only: bool,
    /// Current live activity label ("Running · edit_file bridge.rs").
    live_label: Option<String>,
    /// When each active process group started.
    group_started: HashMap<RowId, chrono::DateTime<chrono::Utc>>,
    /// Elapsed time retained for a settled process group.
    group_elapsed: HashMap<RowId, String>,
    activity_timer: Option<Task<()>>,
    process_roles: Arc<Vec<ProcessRowRole>>,
    process_groups: Arc<Vec<ProcessGroup>>,
    expanded_process_groups: HashSet<RowId>,
    pending_scroll: Rc<Cell<f32>>,
    scroll_scheduled: Rc<Cell<bool>>,
    scroll_task: Rc<RefCell<Option<Task<()>>>>,
}

/// Per-frame inputs the root hands the transcript scroller. Bundled so the
/// setter keeps one argument per concern as the shell grows.
struct TranscriptViewState {
    rows: Vec<Entity<TranscriptRowView>>,
    new_activity: bool,
    wheel_enabled: bool,
    answers_only: bool,
    live_label: Option<String>,
}

impl TranscriptScrollerView {
    fn new(
        scroller: Entity<TranscriptListState>,
        root: WeakEntity<DesktopRoot>,
        session_id: SessionId,
        language: tact_catalog::CatalogLocale,
        tail_following: bool,
    ) -> Self {
        Self {
            scroller,
            rows: Arc::new(Vec::new()),
            root,
            session_id,
            language,
            tail_following,
            new_activity: false,
            wheel_enabled: true,
            answers_only: false,
            live_label: None,
            group_started: HashMap::new(),
            group_elapsed: HashMap::new(),
            activity_timer: None,
            process_roles: Arc::new(Vec::new()),
            process_groups: Arc::new(Vec::new()),
            expanded_process_groups: HashSet::new(),
            pending_scroll: Rc::new(Cell::new(0.)),
            scroll_scheduled: Rc::new(Cell::new(false)),
            scroll_task: Rc::new(RefCell::new(None)),
        }
    }

    fn set_state(
        &mut self,
        state: TranscriptViewState,
        session_id: &SessionId,
        snapshot: &DesktopSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let TranscriptViewState {
            rows,
            new_activity,
            wheel_enabled,
            answers_only,
            live_label,
        } = state;
        let language = snapshot.language;
        let tail_following = snapshot.tail_following;
        let rows_changed = self.rows.len() != rows.len()
            || self
                .rows
                .iter()
                .zip(rows.iter())
                .any(|(current, next)| current.entity_id() != next.entity_id());
        let session_changed = self.session_id != *session_id;
        if session_changed {
            self.session_id = session_id.clone();
            self.expanded_process_groups.clear();
            self.group_started.clear();
            self.group_elapsed.clear();
        }
        if rows_changed {
            self.rows = Arc::new(rows);
        }
        let process_rows = self
            .rows
            .iter()
            .map(|view| view.read(cx).row.clone())
            .collect::<Vec<_>>();
        let (process_roles, process_groups) = build_process_layout(&process_rows);
        let layout_changed = self.process_roles.as_ref() != &process_roles
            || self.process_groups.as_ref() != &process_groups;
        self.process_roles = Arc::new(process_roles);
        self.process_groups = Arc::new(process_groups);
        if !session_changed {
            let now = chrono::Utc::now();
            // A group that pauses between tool batches is not finished: the
            // next thinking chunk reopens it. Keep its clock until the run
            // ends or a newer group takes over.
            let run_active = matches!(
                snapshot.session.phase,
                SessionPhase::Running | SessionPhase::Starting | SessionPhase::Opening
            );
            let current_group = self.process_groups.last().map(|group| group.first_row);
            let mut settled = Vec::new();
            for (row, started) in &self.group_started {
                let active = self
                    .process_groups
                    .iter()
                    .any(|group| group.first_row == *row && group.active);
                let live = run_active && current_group == Some(*row);
                if !active && !live {
                    settled.push((*row, *started));
                }
            }
            for (row, started) in settled {
                self.group_started.remove(&row);
                self.group_elapsed.insert(row, format_elapsed(started));
            }
            for group in self.process_groups.iter().filter(|group| group.active) {
                self.group_started.entry(group.first_row).or_insert(now);
            }
        }
        self.ensure_activity_timer(cx);
        self.expanded_process_groups.retain(|row| {
            self.process_groups
                .iter()
                .any(|group| &group.first_row == row)
        });
        let changed = rows_changed
            || session_changed
            || layout_changed
            || self.language != language
            || self.tail_following != tail_following
            || self.new_activity != new_activity
            || self.wheel_enabled != wheel_enabled
            || self.answers_only != answers_only
            || self.live_label != live_label;
        self.language = language;
        self.tail_following = tail_following;
        self.new_activity = new_activity;
        self.wheel_enabled = wheel_enabled;
        self.answers_only = answers_only;
        self.live_label = live_label;
        if !wheel_enabled {
            self.pending_scroll.set(0.);
        }
        if changed {
            cx.notify();
        }
        changed
    }

    /// Repaints the summary clock once a second while a group's clock runs.
    fn ensure_activity_timer(&mut self, cx: &mut Context<Self>) {
        let live = !self.group_started.is_empty();
        if !live {
            self.activity_timer = None;
            return;
        }
        if self.activity_timer.is_some() {
            return;
        }
        self.activity_timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        !this.group_started.is_empty()
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn toggle_process_group(&mut self, first_row: RowId, cx: &mut Context<Self>) {
        let Some(group) = self
            .process_groups
            .iter()
            .find(|group| group.first_row == first_row)
            .cloned()
        else {
            return;
        };
        if !self.expanded_process_groups.remove(&first_row) {
            self.expanded_process_groups.insert(first_row);
        }
        self.scroller.update(cx, |state, cx| {
            state.remeasure_items(group.first_index..group.end_index, cx);
        });
        cx.notify();
    }

    fn back_to_bottom(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = format!(
            "{} · {}",
            i18n::text(self.language, TextKey::BackToBottom),
            i18n::text(self.language, TextKey::NewActivity)
        );
        let new_activity = self.new_activity;
        h_flex()
            .id("back-to-bottom-strip")
            .absolute()
            .right_4()
            .bottom_4()
            .occlude()
            .items_center()
            .gap(px(6.))
            .when(new_activity, |strip| {
                strip.child(
                    div()
                        .rounded_full()
                        .bg(cx.theme().accent)
                        .text_color(cx.theme().link)
                        .px_3()
                        .py_2()
                        .text_xs()
                        .font_semibold()
                        .child(i18n::text(self.language, TextKey::NewActivity)),
                )
            })
            .child(
                div()
                    .id("back-to-bottom")
                    .debug_selector(|| "back-to-bottom".into())
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(label)
                    .cursor_pointer()
                    .rounded_full()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .shadow_md()
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(5.))
                            .child(i18n::text(self.language, TextKey::BackToBottom))
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.new_activity = false;
                        this.scroller
                            .update(cx, |state, cx| state.scroll_to_end(cx));
                        if let Some(root) = this.root.upgrade() {
                            root.update(cx, |root, cx| {
                                root.new_activity = false;
                                root.dispatch(DesktopIntent::BackToBottom);
                                cx.notify();
                            });
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

/// The summary row's text: the elapsed duration leads, followed by the live
/// action while the group runs or the counts once it settles.
fn process_summary_label(
    group: &ProcessGroup,
    live: Option<&str>,
    elapsed: Option<&str>,
    language: tact_catalog::CatalogLocale,
) -> String {
    if group.active
        && let Some(live) = live
    {
        return match elapsed {
            Some(elapsed) => format!("{elapsed} · {live}"),
            None => live.to_string(),
        };
    }
    let counts = process_summary_text(group, language);
    match elapsed {
        Some(elapsed) => format!("{elapsed} · {counts}"),
        None => counts,
    }
}

fn process_summary_element(
    group: &ProcessGroup,
    expanded: bool,
    language: tact_catalog::CatalogLocale,
    live: Option<&str>,
    elapsed: Option<&str>,
    weak: WeakEntity<TranscriptScrollerView>,
    cx: &gpui::App,
) -> AnyElement {
    let label = process_summary_label(group, live, elapsed, language);
    let first_row = group.first_row;
    let ordinal = group.first_ordinal;
    h_flex()
        .id(SharedString::from(format!("process-summary-{ordinal}")))
        .debug_selector(move || format!("transcript-process-summary-{ordinal}"))
        .role(Role::Button)
        .aria_label(label.clone())
        .aria_expanded(expanded)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .cursor_pointer()
        .gap_1()
        .items_center()
        .rounded(px(6.))
        .px_1()
        .py_1()
        .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
        .child(
            Icon::new(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(cx.theme().muted_foreground),
        )
        .when(group.active, |row| row.child(Orbs::new().diameter(px(12.))))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(13.))
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .on_click(move |_, _, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.toggle_process_group(first_row, cx);
            });
        })
        .into_any_element()
}

impl Render for TranscriptScrollerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let answers_only = self.answers_only;
        let live_label = self.live_label.clone();
        let group_started = self.group_started.clone();
        let group_elapsed = self.group_elapsed.clone();
        let process_roles = self.process_roles.clone();
        let process_groups = self.process_groups.clone();
        let expanded_process_groups = self.expanded_process_groups.clone();
        let language = self.language;
        let process_weak = cx.weak_entity();
        let list_state = self.scroller.read(cx).list_state.clone();
        let item_count = list_state.item_count();
        let list = list(list_state.clone(), move |index, _, cx| {
            if answers_only
                && rows
                    .get(index)
                    .is_some_and(|view| !answers_only_visible(&view.read(cx).row.body))
            {
                return div().into_any_element();
            }
            let role = process_roles
                .get(index)
                .copied()
                .unwrap_or(ProcessRowRole::Normal);
            let process_group = match role {
                ProcessRowRole::Summary(group) | ProcessRowRole::Hidden(group) => {
                    process_groups.get(group)
                }
                ProcessRowRole::Normal => None,
            };
            let process_expanded = process_group
                .is_some_and(|group| expanded_process_groups.contains(&group.first_row));
            let summary_live = process_group
                .filter(|group| group.active)
                .and_then(|_| live_label.clone());
            let summary_elapsed = process_group.and_then(|group| {
                group_started
                    .get(&group.first_row)
                    .map(|started| format_elapsed(*started))
                    .or_else(|| group_elapsed.get(&group.first_row).cloned())
            });
            if matches!(role, ProcessRowRole::Hidden(_)) && !process_expanded {
                return div().into_any_element();
            }
            // Every item owned by a process group uses the stream rhythm,
            // including transparent plan rows that render no content. Letting
            // those invisible items pay the prose gap opened a blank band
            // between consecutive thinking and tool rows.
            let tight_process_gap = !matches!(role, ProcessRowRole::Normal)
                || process_roles
                    .get(index + 1)
                    .is_some_and(|next| !matches!(next, ProcessRowRole::Normal))
                || (rows
                    .get(index)
                    .is_some_and(|view| is_process_row(&view.read(cx).row.body))
                    && rows
                        .get(index + 1)
                        .is_some_and(|view| is_process_row(&view.read(cx).row.body)));
            let row_element = rows
                .get(index)
                .cloned()
                .map(IntoElement::into_any_element)
                .unwrap_or_else(|| div().into_any_element());
            let (content, tight) = match (role, process_group) {
                (ProcessRowRole::Summary(_), Some(group)) if !process_expanded => (
                    process_summary_element(
                        group,
                        false,
                        language,
                        summary_live.as_deref(),
                        summary_elapsed.as_deref(),
                        process_weak.clone(),
                        cx,
                    ),
                    false,
                ),
                (ProcessRowRole::Summary(_), Some(group)) => (
                    v_flex()
                        .w_full()
                        .gap(px(2.))
                        .child(process_summary_element(
                            group,
                            true,
                            language,
                            summary_live.as_deref(),
                            summary_elapsed.as_deref(),
                            process_weak.clone(),
                            cx,
                        ))
                        .child(row_element)
                        .into_any_element(),
                    tight_process_gap,
                ),
                _ => (row_element, tight_process_gap),
            };
            div()
                .w_full()
                .min_w_0()
                .px_3()
                .refine_style(&transcript_row_style())
                .when(index + 1 < item_count, |this| {
                    this.pb(px(if tight { 0. } else { 12. }))
                })
                .child(content)
                .into_any_element()
        })
        .size_full()
        .min_h_0()
        .py_2();
        let viewport = div()
            .id("desktop-transcript")
            .debug_selector(|| "desktop-transcript".to_owned())
            .role(Role::Log)
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(list);
        let scroll_weak = cx.weak_entity();
        let scroll = coalesced_wheel_canvas(
            self.pending_scroll.clone(),
            self.scroll_scheduled.clone(),
            self.scroll_task.clone(),
            move |delta, cx| {
                let _ = scroll_weak.update(cx, |view, cx| {
                    view.scroller.read(cx).list_state.scroll_by(px(-delta));
                    cx.notify();
                });
            },
        );
        let show_back_to_bottom = self.scroller.read(cx).is_scrolled_up();
        div()
            .relative()
            .size_full()
            .min_h_0()
            .child(viewport)
            .when(self.wheel_enabled, |region| region.child(scroll))
            .when(show_back_to_bottom, |region| {
                region.child(self.back_to_bottom(cx))
            })
    }
}

pub(crate) struct DesktopRoot {
    controller: DesktopController,
    snapshot: Arc<DesktopSnapshot>,
    scroller: Entity<TranscriptListState>,
    transcript_scroller: Entity<TranscriptScrollerView>,
    prompt: Entity<PromptBar>,
    sidebar_nav: Entity<SidebarNav>,
    extensions_table: Entity<FilterTable>,
    assistant_selections: HashMap<(String, SessionId, RowId), AssistantSelection>,
    transcript_rows: HashMap<(SessionId, RowId), Entity<TranscriptRowView>>,
    pending_row_updates: HashSet<RowId>,
    /// Thinking rows the reader expanded inline. The `Thinking` component is
    /// controlled, so the shell owns this state and the trace stays in the
    /// transcript instead of opening a reader.
    expanded_thinking: HashSet<(SessionId, RowId)>,
    transcript_session_cache: VecDeque<SessionId>,
    command_search: Entity<CommandSearch>,
    command_scope: Option<CommandId>,
    command_hit_count: usize,
    command_query: String,
    form_input: Entity<gpui_component::input::InputState>,
    criteria_input: Entity<gpui_component::input::InputState>,
    session_search: Entity<gpui_component::input::InputState>,
    extension_search: Entity<gpui_component::input::InputState>,
    queue_focus: gpui::FocusHandle,
    overlay_focus: gpui::FocusHandle,
    overlay_focus_pending: bool,
    focus_restore_pending: bool,
    focus_composer_on_ready: bool,
    _events_task: Option<Task<()>>,
    run_clock_timer: Option<Task<()>>,
    left_visible: bool,
    right_visible: bool,
    overlay: Option<Overlay>,
    overlay_stack: Vec<Overlay>,
    extension_query: String,
    extension_manage: bool,
    slash_overlay: Option<SlashOverlayState>,
    slash_dismissed_for: Option<String>,
    permission_picker_open: bool,
    effort_picker_open: bool,
    model_picker_cursor: usize,
    /// Quick filter for the model pickers. A provider's index can hold dozens
    /// of ids, so the list is searchable and navigable rather than scrolled
    /// by hand.
    model_search: Entity<gpui_component::input::InputState>,
    model_picker_query: String,
    /// Scroll owner for the model rows, so keyboard selection can bring the
    /// highlighted row into view.
    model_picker_scroll: RowScroll,
    /// Bumped whenever an overlay opens, so the entrance animation plays for
    /// each one instead of only for the first.
    overlay_epoch: u64,
    submitted_gates: HashSet<u64>,
    /// Test-only count of intents that actually reached the controller.
    ///
    /// A catalog entry whose match arm returns without dispatching or opening
    /// a surface is visible but inert; the execution-coverage test needs a
    /// signal that does not itself prove the route ran.
    #[cfg(test)]
    dispatched_intents: std::sync::atomic::AtomicUsize,
    /// Test-only count of platform file pickers a route asked the OS to show.
    #[cfg(test)]
    platform_prompts: std::sync::atomic::AtomicUsize,
    queue_cursor: usize,
    queue_expanded: bool,
    /// Hide process rows and keep only the user's messages and final answers.
    answers_only: bool,
    recent_commands: Vec<CommandId>,
    sticky_tasks_collapsed: bool,
    /// First row the expanded strip paints. The strip shows at most
    /// `sticky::MAX_VISIBLE_ROWS` rows, so this is bounded by the list length
    /// rather than free-running.
    sticky_tasks_scroll: usize,
    /// Whether transcript rows arrived while the reader was scrolled away.
    ///
    /// The pill's Back to bottom control is about position; the New activity
    /// cue is about content, so it must not appear merely because a session
    /// is scrolled up or has finished.
    new_activity: bool,
    voice: VoiceUi,
    /// Dictated text waiting for a render pass that owns a `Window`.
    pending_voice_text: Option<String>,
    history_cursor: Option<usize>,
    session_project_filter: Option<ProjectId>,
    collapsed_session_projects: HashSet<ProjectId>,
    reset_session_search_on_open: bool,
    toast: Option<Toast>,
    toast_epoch: u64,
    rail_width: f32,
    inspector_width: f32,
    divider_drag: Option<DividerDrag>,
    focus_restore: crate::ui::shell::FocusRestoration<gpui::FocusHandle>,
    last_surface_revision: Option<String>,
    /// The transcript row whose reader request is in flight.
    pending_reader: Option<ReaderRef>,
    /// Pending decisions currently drawn on the dock/taskbar badge.
    last_badge: usize,
    last_commit_revision: u64,
    last_appearance: Appearance,
    last_theme: DesktopTheme,
    last_language: tact_catalog::CatalogLocale,
    mentions_root: Option<PathBuf>,
    prompt_mentions: Vec<PromptMention>,
    sidebar_query: String,
    rail_search: Entity<gpui_component::input::InputState>,
    _subscriptions: Vec<gpui::Subscription>,
    attachments: Vec<PathBuf>,
    /// Which page of the run room is showing: the conversation or the
    /// trajectory ledger.
    run_tab: RunTab,
    /// Cached projection of the current transcript for the Trajectory page.
    ///
    /// The projection is pure, so it is rebuilt only when the transcript's
    /// length or revision changes rather than on every frame.
    trajectory: RefCell<Option<(u64, Arc<TrajectoryView>)>>,
    /// The record the Trajectory detail panel is showing.
    trajectory_selected: Option<TrajectoryRecordId>,
    /// Which tab the detail panel is on.
    trajectory_tab: TrajectoryDetailTab,
    /// Live search over the ledger. Empty means "show everything".
    trajectory_query: String,
    trajectory_search: Entity<gpui_component::input::InputState>,
    /// Scroll owner for the ledger list, shared with the capture harness.
    trajectory_scroller: Entity<TranscriptListState>,
    /// Item count the ledger list was last reset to.
    trajectory_row_count: Rc<Cell<usize>>,
    /// The selected record's list position, so the ledger can paint it without
    /// re-walking the projection.
    trajectory_selected_index: Option<usize>,
    /// The overview plot's painted track origin and width, written by the
    /// canvas prepaint so a click maps back to the block it landed on.
    trajectory_track: Rc<Cell<(f32, f32)>>,
    /// Scroll owner for the detail panel's body, so a tool result longer than
    /// the column scrolls instead of painting past it.
    trajectory_detail_scroll: gpui::ScrollHandle,
}

impl DesktopRoot {
    pub(crate) fn new(
        controller: DesktopController,
        events: tact_desktop_core::DesktopEvents,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_inner(controller, Some(events), window, cx)
    }

    #[cfg(any(test, feature = "ui-capture"))]
    pub(crate) fn new_for_test(
        controller: DesktopController,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_inner(controller, None, window, cx)
    }

    fn new_inner(
        controller: DesktopController,
        events: Option<tact_desktop_core::DesktopEvents>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshot = controller.snapshot();
        let initial_language = snapshot.language;
        let voice = VoiceUi {
            enabled: snapshot.voice_enabled,
            missing_api_key: snapshot.voice_missing_api_key,
            listening: false,
            transcribing: false,
        };
        let scroller = cx.new(|_| TranscriptListState::new(snapshot.transcript.len()));
        let root_weak = cx.weak_entity();
        let initial_session_id = snapshot
            .session
            .id
            .clone()
            .unwrap_or_else(|| SessionId(format!("epoch-{}", snapshot.session.epoch.0)));
        let transcript_scroller = cx.new(|_| {
            TranscriptScrollerView::new(
                scroller.clone(),
                root_weak.clone(),
                initial_session_id,
                snapshot.language,
                snapshot.tail_following,
            )
        });
        let prompt = cx.new(|cx| PromptBar::new("tact-composer", window, cx));
        let trailing_weak = root_weak.clone();
        prompt.update(cx, |prompt, cx| {
            prompt.set_leading_controls(
                move |window, cx| {
                    let Some(root) = root_weak.upgrade() else {
                        return Vec::new();
                    };
                    root.update(cx, |root, cx| root.render_composer_leading(window, cx))
                },
                cx,
            );
            prompt.set_trailing_controls(
                move |window, cx| {
                    let Some(root) = trailing_weak.upgrade() else {
                        return Vec::new();
                    };
                    root.update(cx, |root, cx| root.render_composer_trailing(window, cx))
                },
                cx,
            );
            prompt.set_attach_visible(false, cx);
            prompt.set_enhance_visible(false, cx);
        });
        let sidebar_nav = cx.new(|cx| {
            SidebarNav::new("tact-session-nav", window, cx)
                .with_presentation(SidebarNavPresentation::Embedded)
                // The rail is chrome, not component content: the design's
                // rows are 12px over 10px with a monospace time, not the
                // component library's 14px over 12px.
                .with_compact_rows(true)
        });
        let extensions_table = cx.new(|cx| {
            FilterTable::new(
                "tact-extensions-table",
                i18n::text(snapshot.language, TextKey::Extensions),
                window,
                cx,
            )
        });
        let command_search = cx.new(|cx| CommandSearch::new("tact-command-center", window, cx));
        let form_input = cx.new(|cx| gpui_component::input::InputState::new(window, cx));
        let criteria_input = cx.new(|cx| gpui_component::input::InputState::new(window, cx));
        let session_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(i18n::text(snapshot.language, TextKey::SearchSessions))
        });
        let extension_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(i18n::text(snapshot.language, TextKey::SearchExtensions))
        });
        let rail_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(rail_search_placeholder(snapshot.language))
        });
        let model_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(i18n::text(snapshot.language, TextKey::SearchModels))
        });
        let trajectory_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(i18n::text(snapshot.language, TextKey::SearchTrajectory))
        });
        let trajectory_scroller = cx.new(|_| TranscriptListState::new(0));
        let queue_focus = cx.focus_handle();
        let overlay_focus = cx.focus_handle();

        let trajectory_search_subscription = cx.subscribe_in(
            &trajectory_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, window, cx| match event {
                gpui_component::input::InputEvent::Change => {
                    let query = this.trajectory_search.read(cx).value().to_string();
                    this.set_trajectory_query(query, window, cx);
                }
                gpui_component::input::InputEvent::Focus
                | gpui_component::input::InputEvent::Blur => cx.notify(),
                gpui_component::input::InputEvent::PressEnter { .. } => {}
            },
        );
        let editor_subscription = cx.subscribe_in(
            &prompt,
            window,
            |this, prompt, event: &PromptBarEvent, window, cx| {
                this.on_prompt_event(prompt, event, window, cx);
            },
        );
        let search_subscription = cx.subscribe_in(
            &command_search,
            window,
            |this, _, event: &CommandSearchEvent, window, cx| {
                this.on_command_search(event, window, cx);
            },
        );
        let sidebar_subscription = cx.subscribe_in(
            &sidebar_nav,
            window,
            |this, _, event: &SidebarNavEvent, window, cx| match event {
                SidebarNavEvent::Selected { item_id, .. } => {
                    this.dispatch(DesktopIntent::ResumeSession {
                        session_id: SessionId(item_id.to_string()),
                    });
                }
                SidebarNavEvent::NewTaskRequested { .. } => {
                    this.dispatch(DesktopIntent::NewSession);
                }
                SidebarNavEvent::QueryChanged { query, .. } => {
                    this.sidebar_query = query.to_string();
                    cx.notify();
                }
                SidebarNavEvent::SectionToggled {
                    section_id,
                    collapsed,
                    ..
                } => {
                    if let Some(project_id) = section_id.strip_prefix("sessions-project-") {
                        let project_id = ProjectId(project_id.to_string());
                        if *collapsed {
                            this.collapsed_session_projects.insert(project_id);
                        } else {
                            this.collapsed_session_projects.remove(&project_id);
                        }
                        cx.notify();
                    }
                }
                SidebarNavEvent::SectionActionRequested { section_id, .. } => {
                    if let Some(project_id) = section_id.strip_prefix("sessions-project-") {
                        this.open_remove_project(ProjectId(project_id.to_string()), window, cx);
                    }
                }
                SidebarNavEvent::SectionNewSessionRequested { section_id, .. } => {
                    if let Some(project_id) = section_id.strip_prefix("sessions-project-") {
                        this.dispatch(DesktopIntent::NewSessionInProject {
                            project_id: ProjectId(project_id.to_string()),
                        });
                    }
                }
                SidebarNavEvent::ItemActionRequested { item_id, .. } => {
                    if let Some(session) = this
                        .snapshot
                        .sessions
                        .items
                        .iter()
                        .find(|session| session.id == item_id.as_ref())
                    {
                        let archived = session.archived_at.is_none();
                        this.dispatch(DesktopIntent::SetSessionArchived {
                            session_id: SessionId(item_id.to_string()),
                            archived,
                        });
                    }
                }
                SidebarNavEvent::ItemNewSessionRequested { item_id, .. } => {
                    let project_id = this
                        .snapshot
                        .sessions
                        .items
                        .iter()
                        .find(|session| session.id == item_id.as_ref())
                        .map(|session| session.project_id.clone());
                    if let Some(project_id) = project_id {
                        this.dispatch(DesktopIntent::NewSessionInProject { project_id });
                    }
                }
                SidebarNavEvent::CollapsedChanged { .. } => {}
            },
        );
        let rail_search_subscription = cx.subscribe_in(
            &rail_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, window, cx| match event {
                gpui_component::input::InputEvent::Change => {
                    let query = this.rail_search.read(cx).value().to_string();
                    this.sidebar_query = query.clone();
                    this.sidebar_nav.update(cx, |nav, cx| {
                        nav.set_query(query, window, cx);
                    });
                    cx.notify();
                }
                gpui_component::input::InputEvent::Focus
                | gpui_component::input::InputEvent::Blur => cx.notify(),
                gpui_component::input::InputEvent::PressEnter { .. } => {}
            },
        );
        let extensions_table_subscription = cx.subscribe_in(
            &extensions_table,
            window,
            |this, _, event: &FilterTableEvent, window, cx| match event {
                FilterTableEvent::FilterRequested { filter_id, .. } => {
                    let tab = match filter_id.as_ref() {
                        "mcp" => ExtensionsTab::Mcp,
                        "skills" => ExtensionsTab::Skills,
                        "plugins" => ExtensionsTab::Plugins,
                        "hooks" => ExtensionsTab::Hooks,
                        _ => return,
                    };
                    if let Some(Overlay::Extensions(current)) = &mut this.overlay {
                        *current = tab;
                    } else {
                        this.open_overlay(Overlay::Extensions(tab));
                    }
                    this.extension_manage = true;
                    let id = match tab {
                        ExtensionsTab::Mcp => CommandId::Builtin(BuiltinCommand::Mcp),
                        ExtensionsTab::Skills => CommandId::Builtin(BuiltinCommand::Skill),
                        ExtensionsTab::Plugins => CommandId::Builtin(BuiltinCommand::Plugin),
                        ExtensionsTab::Hooks => CommandId::Builtin(BuiltinCommand::Hooks),
                    };
                    this.dispatch(DesktopIntent::InvokeCommand {
                        id,
                        args: CommandArguments::default(),
                    });
                    cx.notify();
                }
                FilterTableEvent::SelectionRequested { row_id, .. } => {
                    this.extensions_table.update(cx, |table, cx| {
                        table.set_selected_row(row_id.clone(), window, cx);
                    });
                }
                FilterTableEvent::ActivationRequested { row_id, source, .. } => {
                    let mut parts = row_id.splitn(3, '|');
                    let kind = parts.next().unwrap_or_default();
                    let action = parts.next().unwrap_or_default();
                    let target = parts.next().unwrap_or_default();
                    match (kind, action) {
                        ("mcp", "auth") => {
                            this.dispatch(DesktopIntent::InvokeCommand {
                                id: CommandId::BuiltinPath {
                                    root: BuiltinCommand::Mcp,
                                    path: vec!["mcp", "auth"],
                                },
                                args: CommandArguments {
                                    tokens: vec![target.to_string()],
                                },
                            });
                        }
                        ("skill", "toggle") => {
                            if matches!(source, ActivationSource::Space) {
                                this.dispatch(DesktopIntent::SetSkillEnabled {
                                    name: target.to_string(),
                                    enabled: !this
                                        .snapshot
                                        .extensions
                                        .snapshot
                                        .skills
                                        .iter()
                                        .any(|skill| skill.name == target && skill.enabled),
                                });
                            } else {
                                this.insert_skill(target, window, cx);
                            }
                        }
                        ("plugin", "update") => {
                            this.dispatch(DesktopIntent::InvokeCommand {
                                id: CommandId::Cli(tact_catalog::CliEntry::PluginUpdate),
                                args: CommandArguments {
                                    tokens: vec![target.to_string()],
                                },
                            });
                        }
                        ("hook", "trust") => {
                            this.dispatch(DesktopIntent::InvokeCommand {
                                id: CommandId::Cli(tact_catalog::CliEntry::HooksTrust),
                                args: CommandArguments {
                                    tokens: vec!["--source".into(), target.to_string()],
                                },
                            });
                        }
                        _ => {}
                    }
                }
                FilterTableEvent::SortRequested { .. } => {}
            },
        );
        let session_search_subscription = cx.subscribe_in(
            &session_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, _, cx| match event {
                gpui_component::input::InputEvent::Change => {
                    let query = this.session_search.read(cx).value().to_string();
                    this.dispatch(DesktopIntent::SetSessionQuery(query));
                }
                gpui_component::input::InputEvent::Focus
                | gpui_component::input::InputEvent::Blur => cx.notify(),
                gpui_component::input::InputEvent::PressEnter { .. } => {}
            },
        );
        let extension_search_subscription = cx.subscribe_in(
            &extension_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, window, cx| match event {
                gpui_component::input::InputEvent::Change => {
                    this.extension_query = this.extension_search.read(cx).value().to_string();
                    cx.notify();
                }
                gpui_component::input::InputEvent::PressEnter { .. } => {
                    this.activate_extension_search_result(window, cx);
                }
                gpui_component::input::InputEvent::Focus
                | gpui_component::input::InputEvent::Blur => cx.notify(),
            },
        );
        let model_search_subscription = cx.subscribe_in(
            &model_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, _, cx| {
                match event {
                    gpui_component::input::InputEvent::Change => {
                        this.model_picker_query = this.model_search.read(cx).value().to_string();
                        // A query change reorders nothing but rebuilds the visible
                        // set, so the old cursor would point at a different model.
                        this.model_picker_cursor = 0;
                        this.model_picker_scroll.reset();
                        cx.notify();
                    }
                    gpui_component::input::InputEvent::Focus
                    | gpui_component::input::InputEvent::Blur => cx.notify(),
                    _ => {}
                }
            },
        );

        let events_task = events.map(|events| {
            cx.spawn(async move |this: WeakEntity<Self>, cx| {
                while let Ok(event) = events.recv().await {
                    if this
                        .update(cx, |this, cx| this.handle_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });

        let initial_revision = snapshot.revision;
        let mut root = Self {
            controller,
            snapshot,
            scroller,
            transcript_scroller,
            prompt,
            sidebar_nav,
            extensions_table,
            assistant_selections: HashMap::new(),
            transcript_rows: HashMap::new(),
            pending_row_updates: HashSet::new(),
            expanded_thinking: HashSet::new(),
            transcript_session_cache: VecDeque::new(),
            command_search,
            command_scope: None,
            command_hit_count: 0,
            command_query: String::new(),
            form_input,
            criteria_input,
            session_search,
            extension_search,
            queue_focus,
            overlay_focus,
            overlay_focus_pending: false,
            focus_restore_pending: false,
            focus_composer_on_ready: false,
            _events_task: events_task,
            run_clock_timer: None,
            left_visible: true,
            right_visible: false,
            overlay: None,
            overlay_stack: Vec::new(),
            extension_query: String::new(),
            extension_manage: false,
            slash_overlay: None,
            slash_dismissed_for: None,
            permission_picker_open: false,
            effort_picker_open: false,
            model_picker_cursor: 0,
            model_search,
            model_picker_query: String::new(),
            model_picker_scroll: RowScroll::default(),
            overlay_epoch: 0,
            submitted_gates: HashSet::new(),
            #[cfg(test)]
            dispatched_intents: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            platform_prompts: std::sync::atomic::AtomicUsize::new(0),
            queue_cursor: 0,
            queue_expanded: false,
            answers_only: false,
            recent_commands: Vec::new(),
            sticky_tasks_collapsed: true,
            sticky_tasks_scroll: 0,
            new_activity: false,
            voice,
            pending_voice_text: None,
            history_cursor: None,
            session_project_filter: None,
            collapsed_session_projects: HashSet::new(),
            reset_session_search_on_open: false,
            toast: None,
            toast_epoch: 0,
            rail_width: crate::ui::layout::RAIL_WIDTH,
            inspector_width: crate::ui::layout::INSPECTOR_WIDTH,
            divider_drag: None,
            focus_restore: crate::ui::shell::FocusRestoration::default(),
            last_surface_revision: None,
            pending_reader: None,
            last_badge: 0,
            last_commit_revision: initial_revision,
            last_appearance: Appearance::Light,
            last_theme: DesktopTheme::Tact,
            last_language: initial_language,
            mentions_root: None,
            prompt_mentions: Vec::new(),
            sidebar_query: String::new(),
            rail_search,
            _subscriptions: vec![
                editor_subscription,
                search_subscription,
                sidebar_subscription,
                rail_search_subscription,
                extensions_table_subscription,
                session_search_subscription,
                extension_search_subscription,
                model_search_subscription,
                trajectory_search_subscription,
            ],
            attachments: Vec::new(),
            run_tab: RunTab::Chat,
            trajectory: RefCell::new(None),
            trajectory_selected: None,
            trajectory_tab: TrajectoryDetailTab::Summary,
            trajectory_query: String::new(),
            trajectory_search,
            trajectory_scroller,
            trajectory_row_count: Rc::new(Cell::new(0)),
            trajectory_selected_index: None,
            trajectory_track: Rc::new(Cell::new((0., 0.))),
            trajectory_detail_scroll: gpui::ScrollHandle::new(),
        };
        root.last_appearance = root.snapshot.appearance;
        root.last_theme = root.snapshot.theme;
        root.sync_prompt_commands(cx);
        root.sync_prompt_state(cx);
        root.rebuild_command_search(window, cx, "");
        let weak = cx.weak_entity();
        let keyboard_subscription = cx.intercept_keystrokes(move |event, window, cx| {
            let handled = weak
                .update(cx, |this, cx| {
                    this.handle_intercepted_key(
                        &event.keystroke.key,
                        event.keystroke.modifiers.shift,
                        window,
                        cx,
                    )
                })
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });
        root._subscriptions.push(keyboard_subscription);
        root
    }

    fn dispatch(&self, intent: DesktopIntent) {
        #[cfg(test)]
        self.dispatched_intents
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.controller.dispatch(intent);
    }

    fn select_inspector(&mut self, tab: InspectorTab, cx: &mut Context<Self>) {
        self.right_visible = true;
        if self.snapshot.inspector.tab != tab {
            let mut snapshot = (*self.snapshot).clone();
            snapshot.inspector.tab = tab;
            self.snapshot = Arc::new(snapshot);
        }
        self.dispatch(DesktopIntent::SetInspector(tab));
        cx.notify();
    }

    fn text(&self, key: TextKey) -> &'static str {
        i18n::text(self.snapshot.language, key)
    }

    /// Switch the run room between the conversation and the ledger.
    ///
    /// Entering the ledger also gives it focus for scrolling, so the wheel
    /// addresses the page the reader is looking at rather than the chat
    /// transcript behind it.
    fn select_run_tab(&mut self, tab: RunTab, cx: &mut Context<Self>) {
        if self.run_tab == tab {
            return;
        }
        self.run_tab = tab;
        if tab == RunTab::Trajectory {
            self.sync_trajectory_scroller(cx);
        }
        cx.notify();
    }

    /// The current transcript projected as a ledger, rebuilt only when the
    /// transcript changed.
    fn trajectory_view(&self) -> Arc<TrajectoryView> {
        let revision = self.snapshot.revision;
        if let Some((cached, view)) = self.trajectory.borrow().as_ref()
            && *cached == revision
        {
            return view.clone();
        }
        let view = Arc::new(TrajectoryView::from_transcript(&self.snapshot.transcript));
        *self.trajectory.borrow_mut() = Some((revision, view.clone()));
        view
    }

    /// Update the ledger's live filter and keep the list in step.
    fn set_trajectory_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.trajectory_query == query {
            return;
        }
        self.trajectory_query = query;
        if self.trajectory_selected.is_some() && !self.trajectory_matches_selected() {
            self.trajectory_selected = None;
        }
        self.sync_trajectory_scroller(cx);
        let _ = window;
        cx.notify();
    }

    fn trajectory_matches_selected(&self) -> bool {
        let view = self.trajectory_view();
        let visible = self.trajectory_visible_indexes(&view);
        match (self.trajectory_selected, visible) {
            (None, _) => true,
            (Some(id), None) => view.position_of(id).is_some(),
            (Some(id), Some(indexes)) => view
                .position_of(id)
                .is_some_and(|position| indexes.contains(&(position + 1))),
        }
    }

    /// 1-based record indexes the live query keeps, or `None` for "all".
    fn trajectory_visible_indexes(&self, view: &TrajectoryView) -> Option<Vec<usize>> {
        if self.trajectory_query.trim().is_empty() {
            return None;
        }
        let matches = view.matches(&self.trajectory_query);
        Some(
            matches
                .into_iter()
                .map(|position| position + 1)
                .collect::<Vec<_>>(),
        )
    }

    /// One list item per ledger row: a turn header or a record.
    fn trajectory_list_rows(&self, view: &TrajectoryView) -> Vec<trajectory_page::LedgerRow> {
        let visible = self.trajectory_visible_indexes(view);
        trajectory_page::ledger_rows(view, visible.as_deref())
    }

    /// Keep the ledger's list item count in step with its projection. A turn
    /// header is a property of the record that opens the turn, so every list
    /// item is one record and the count is the projection's length.
    fn sync_trajectory_scroller(&mut self, cx: &mut Context<Self>) {
        let view = self.trajectory_view();
        let target = self.trajectory_list_rows(&view).len();
        if self.trajectory_row_count.get() == target {
            return;
        }
        self.trajectory_row_count.set(target);
        self.trajectory_scroller
            .update(cx, |state, cx| state.reset(target, cx));
    }

    /// Select one ledger record and reveal it in the ledger.
    fn select_trajectory_record(&mut self, id: TrajectoryRecordId, cx: &mut Context<Self>) -> bool {
        let view = self.trajectory_view();
        if view.position_of(id).is_none() {
            return false;
        }
        self.trajectory_selected = Some(id);
        let rows = self.trajectory_list_rows(&view);
        if let Some(index) = rows.iter().position(|row| row.id == id) {
            self.trajectory_selected_index = Some(index);
            self.trajectory_scroller
                .update(cx, |state, cx| state.scroll_to_item(index, cx));
        }
        cx.notify();
        true
    }

    /// The record the detail panel shows, cloned so the render pass can hold
    /// it while it mutates the root.
    fn trajectory_selected_record(
        &self,
        view: &TrajectoryView,
    ) -> Option<tact_desktop_core::TrajectoryRecord> {
        let id = self.trajectory_selected?;
        let position = view.position_of(id)?;
        view.record(position).cloned()
    }

    fn session_title(&self, session: &tact_desktop_core::SessionSummaryView) -> String {
        session_title_label(
            self.snapshot.language,
            &session.title,
            session.message_count,
        )
    }

    fn toggle_session_project(&mut self, project_id: ProjectId) {
        if !self.collapsed_session_projects.remove(&project_id) {
            self.collapsed_session_projects.insert(project_id);
        }
    }

    fn sync_sidebar_nav(&self, window: &mut Window, cx: &mut Context<Self>) {
        let today = chrono::Local::now().date_naive();
        let mut groups: Vec<(ProjectId, String, String, Vec<SidebarNavItem>)> = Vec::new();
        // `sessions.selected` is set when the click lands, before the new host
        // has produced a snapshot. Prefer it so the rail answers immediately;
        // `session.id` still covers the startup frame where no row is selected.
        let selected = self.snapshot.sessions.selected.as_ref();
        let active_id = selected.or(self.snapshot.session.id.as_ref());
        let new_session_label = self.text(TextKey::NewSession).to_string();
        for session in &self.snapshot.sessions.items {
            let title = self.session_title(session);
            let locked = session.locked && active_id != Some(&SessionId(session.id.clone()));
            let detail = crate::ui::session_rail::session_detail(
                session,
                chrono::Utc::now(),
                self.snapshot.language,
            );
            let updated = session.updated_at.with_timezone(&chrono::Local);
            let trailing = if updated.date_naive() >= today {
                updated.format("%H:%M").to_string()
            } else {
                updated.format("%b %-d").to_string()
            };
            let project_name = self
                .snapshot
                .projects
                .iter()
                .find(|project| project.id.as_ref() == Some(&session.project_id))
                .map(project_identity_label)
                .or_else(|| {
                    (self.snapshot.project.id.as_ref() == Some(&session.project_id))
                        .then(|| project_identity_label(&self.snapshot.project))
                })
                .or_else(|| {
                    session
                        .root_dir
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| self.text(TextKey::Projects).to_string());
            let item = SidebarNavItem::new(session.id.clone(), title)
                .detail(detail)
                .trailing(trailing);
            let item = if locked {
                item.disabled(true)
                    .badge(self.text(TextKey::Locked).to_string())
                    .new_session_action(new_session_label.clone())
            } else {
                item.context_action(
                    self.text(if session.archived_at.is_some() {
                        TextKey::Restore
                    } else {
                        TextKey::Archive
                    })
                    .to_string(),
                )
                .new_session_action(new_session_label.clone())
            };
            let group_id = format!("sessions-project-{}", session.project_id.0);
            if let Some((_, _, _, items)) = groups
                .iter_mut()
                .find(|(project_id, _, _, _)| project_id == &session.project_id)
            {
                items.push(item);
            } else {
                groups.push((
                    session.project_id.clone(),
                    group_id,
                    project_name,
                    vec![item],
                ));
            }
        }
        let (labels, active) = (
            sidebar_nav_labels(self.snapshot.language),
            active_id.map(|id| id.0.clone()),
        );
        let mut sections: Vec<SidebarSection> = groups
            .into_iter()
            .map(|(project_id, id, label, items)| {
                let count = items.len();
                SidebarSection::new(SharedString::from(id), SharedString::from(label.clone()))
                    .leading(project_initials(&label))
                    .meta(count.to_string())
                    .action(self.text(TextKey::RemoveProject))
                    .new_session_action(new_session_label.clone())
                    .collapsed(self.collapsed_session_projects.contains(&project_id))
                    .items(items)
            })
            .collect();
        if sections.is_empty() {
            sections.push(
                SidebarSection::new("sessions", self.text(TextKey::Sessions)).items(Vec::new()),
            );
        }
        self.sidebar_nav.update(cx, |nav, cx| {
            nav.set_labels(labels, window, cx);
            nav.set_chrome_visible(false, cx);
            nav.set_sections(sections, cx);
            if let Some(active) = active {
                nav.set_active_item(active, cx);
            } else {
                nav.clear_active_item(cx);
            }
        });
    }

    fn activate_extension_search_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.overlay.as_ref(),
            Some(Overlay::Extensions(ExtensionsTab::Skills))
        ) {
            self.insert_first_matching_skill(window, cx);
        }
    }

    fn insert_first_matching_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.extension_query.trim().to_string();
        let mut matched = self
            .snapshot
            .extensions
            .snapshot
            .skills
            .iter()
            .filter(|skill| {
                query.is_empty()
                    || crate::ui::extensions::matches_extension(&query, &skill.name)
                    || crate::ui::extensions::matches_extension(&query, &skill.description)
            })
            .collect::<Vec<_>>();
        matched.sort_by_key(|skill| {
            -crate::ui::extensions::score_extension(&query, &[&skill.name, &skill.description])
        });
        if let Some(name) = matched.first().map(|skill| skill.name.clone()) {
            self.insert_skill(&name, window, cx);
        }
    }

    fn insert_skill(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.overlay_stack.clear();
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_draft_at_end(format!("/skill {name} "), window, cx);
            prompt.focus(window, cx);
        });
        self.slash_overlay = None;
        self.slash_dismissed_for = None;
        cx.notify();
    }

    fn sync_extensions_table(
        &self,
        tab: ExtensionsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let locale = self.snapshot.language;
        let (mut mcp, mut skills, mut plugins, mut hooks) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        if let Some(SurfaceSnapshot::McpServers(servers)) = &self.snapshot.surface {
            for server in servers {
                let status = mcp_status_label(&server.status, locale);
                let action = if matches!(
                    server.status,
                    tact_desktop_core::McpServerStatusSnapshot::NeedsAuthorization
                ) {
                    "auth"
                } else {
                    "noop"
                };
                let row =
                    FilterRow::new(format!("mcp|{action}|{}", server.name), server.name.clone())
                        .cells([
                            FilterCell::new("name", truncate_table_text(&server.name, 32)),
                            FilterCell::new("detail", truncate_table_text(&server.transport, 84)),
                            FilterCell::new("status", status),
                        ]);
                mcp.push(row);
            }
        }
        for skill in &self.snapshot.extensions.snapshot.skills {
            let status = if skill.enabled {
                self.text(TextKey::Enabled)
            } else {
                self.text(TextKey::Disabled)
            };
            skills.push(
                FilterRow::new(format!("skill|toggle|{}", skill.name), skill.name.clone()).cells([
                    FilterCell::new("name", truncate_table_text(&skill.name, 32)),
                    FilterCell::new("detail", truncate_table_text(&skill.description, 84)),
                    FilterCell::new("status", status),
                ]),
            );
        }
        for plugin in &self.snapshot.extensions.snapshot.plugins {
            plugins.push(
                FilterRow::new(
                    format!("plugin|update|{}", plugin.name),
                    plugin.name.clone(),
                )
                .cells([
                    FilterCell::new("name", truncate_table_text(&plugin.name, 32)),
                    FilterCell::new("detail", truncate_table_text(&plugin.marketplace, 84)),
                    FilterCell::new(
                        "status",
                        plugin.version.clone().unwrap_or_else(|| "—".into()),
                    ),
                ]),
            );
        }
        if let Some(SurfaceSnapshot::Hooks(snapshot)) = &self.snapshot.surface {
            for hook in snapshot.trusted.iter().chain(snapshot.pending.iter()) {
                hooks.push(
                    FilterRow::new(format!("hook|trust|{}", hook.source), hook.source.clone())
                        .cells([
                            FilterCell::new("name", hook.source.clone()),
                            FilterCell::new("detail", hook.command.clone()),
                            FilterCell::new("status", hook.event.clone()),
                        ]),
                );
            }
        }
        let query = self.extension_query.trim();
        let row_matches = |row: &FilterRow| {
            query.is_empty()
                || crate::ui::extensions::matches_extension(query, row.label())
                || row
                    .all_cells()
                    .iter()
                    .any(|cell| crate::ui::extensions::matches_extension(query, cell.value()))
        };
        mcp.retain(row_matches);
        skills.retain(row_matches);
        plugins.retain(row_matches);
        hooks.retain(row_matches);
        let score = |row: &FilterRow| {
            if query.is_empty() {
                return 0;
            }
            let mut best = crate::ui::extensions::score_extension(query, &[row.label()]);
            for cell in row.all_cells() {
                best = best.max(crate::ui::extensions::score_extension(
                    query,
                    &[cell.value()],
                ));
            }
            best
        };
        mcp.sort_by_key(|row| -score(row));
        skills.sort_by_key(|row| -score(row));
        plugins.sort_by_key(|row| -score(row));
        hooks.sort_by_key(|row| -score(row));
        let filters = [
            FilterDefinition::new("mcp", self.text(TextKey::Mcp), mcp.len())
                .active(tab == ExtensionsTab::Mcp),
            FilterDefinition::new("skills", self.text(TextKey::Skills), skills.len())
                .active(tab == ExtensionsTab::Skills),
            FilterDefinition::new("plugins", self.text(TextKey::Plugins), plugins.len())
                .active(tab == ExtensionsTab::Plugins),
            FilterDefinition::new("hooks", self.text(TextKey::Hooks), hooks.len())
                .active(tab == ExtensionsTab::Hooks),
        ];
        let rows = match tab {
            ExtensionsTab::Mcp => mcp,
            ExtensionsTab::Skills => skills,
            ExtensionsTab::Plugins => plugins,
            ExtensionsTab::Hooks => hooks,
        };
        let has_rows = !rows.is_empty();
        let extension_label = self.text(TextKey::Extensions);
        let (name_label, detail_label, status_label) = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => ("Name", "Detail", "Status"),
            tact_catalog::CatalogLocale::Zh => ("名称", "详情", "状态"),
        };
        let open_label = if tab == ExtensionsTab::Skills {
            match self.snapshot.language {
                tact_catalog::CatalogLocale::En => "Insert",
                tact_catalog::CatalogLocale::Zh => "插入",
            }
        } else {
            self.text(TextKey::Open)
        };
        let filters_label = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => {
                format!("Filters for {}", self.text(TextKey::Extensions))
            }
            tact_catalog::CatalogLocale::Zh => {
                format!("筛选：{}", self.text(TextKey::Extensions))
            }
        };
        let active_label = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => "selected",
            tact_catalog::CatalogLocale::Zh => "已选中",
        };
        let inactive_label = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => "not selected",
            tact_catalog::CatalogLocale::Zh => "未选中",
        };
        let empty_label = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => "No matching records",
            tact_catalog::CatalogLocale::Zh => "没有匹配记录",
        };
        self.extensions_table.update(cx, |table, cx| {
            table.set_label(extension_label, cx);
            table.set_filter_labels(filters_label, active_label, inactive_label, cx);
            table.set_activation_label(open_label, cx);
            table.set_empty_label(empty_label, cx);
            table.set_filters(filters, cx);
            table.set_columns(
                [
                    FilterColumn::new("name", name_label).fill(1.0),
                    FilterColumn::new("detail", detail_label).fill(2.0),
                    FilterColumn::new("status", status_label),
                ],
                window,
                cx,
            );
            table.set_rows(Arc::from(rows), cx);
        });
        has_rows
    }

    fn remember_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = window.focused(cx) {
            self.focus_restore.remember(focus);
        }
    }

    fn open_overlay(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::Extensions(_)) {
            self.extension_manage = false;
        }
        if let Some(current) = self.overlay.take() {
            self.overlay_stack.push(current);
        }
        self.overlay = Some(overlay);
        // Keys the entrance animation, so every opening plays it once
        // instead of only the first one for the lifetime of the element.
        self.overlay_epoch = self.overlay_epoch.wrapping_add(1);
        self.overlay_focus_pending = true;
    }

    fn close_overlay(&mut self) {
        if matches!(self.overlay, Some(Overlay::SessionManager)) {
            // The manager's query is controller state, not overlay-local
            // state. Closing the manager must not leave the rail silently
            // filtered behind an empty rail-search field.
            self.dispatch(DesktopIntent::SetSessionQuery(String::new()));
            self.reset_session_search_on_open = true;
        }
        self.overlay = self.overlay_stack.pop();
    }

    /// The overlay's variant as a comparable label.
    ///
    /// `Overlay` carries state that is not `PartialEq`, and a test that asks
    /// "did this command open, switch, or close a surface?" only needs the
    /// shape of the overlay, not its payload.
    #[cfg(test)]
    fn overlay_kind(&self) -> Option<&'static str> {
        self.overlay.as_ref().map(|overlay| match overlay {
            Overlay::CommandCenter => "command-center",
            Overlay::Settings => "settings",
            Overlay::Form(_) => "form",
            Overlay::Reader(_) => "reader",
            Overlay::Subagent(_) => "subagent",
            Overlay::ModelPicker(_) => "model-picker",
            Overlay::Extensions(_) => "extensions",
            Overlay::Hooks => "hooks",
            Overlay::SessionManager => "session-manager",
        })
    }

    fn handle_event(&mut self, event: DesktopEvent, cx: &mut Context<Self>) {
        match event {
            DesktopEvent::Committed(commit) => {
                let old_rows = self.scroller.read(cx).item_count();
                let scrolled_up = self.scroller.read(cx).is_scrolled_up();
                let transcript_changed = commit.sections.contains(SectionMask::TRANSCRIPT)
                    && (commit.snapshot.transcript.len() != self.snapshot.transcript.len()
                        || !commit.dirty_rows.is_empty());
                let running = matches!(
                    commit.snapshot.session.phase,
                    SessionPhase::Running | SessionPhase::Starting | SessionPhase::Opening
                );
                let boundary = commit.revision > self.last_commit_revision.saturating_add(1)
                    || self.snapshot.session.epoch != commit.snapshot.session.epoch;
                self.new_activity = next_new_activity(
                    self.new_activity,
                    scrolled_up,
                    self.snapshot.tail_following,
                    transcript_changed,
                    running,
                    boundary,
                );
                let commit_gap = commit.revision > self.last_commit_revision.saturating_add(1);
                let epoch_changed = self.snapshot.session.epoch != commit.snapshot.session.epoch;
                if epoch_changed {
                    self.pending_row_updates.clear();
                }
                self.pending_row_updates
                    .extend(commit.dirty_rows.iter().copied());
                if commit_gap && !epoch_changed {
                    self.pending_row_updates
                        .extend(commit.snapshot.transcript.iter().map(|row| row.id));
                }
                if epoch_changed {
                    self.submitted_gates.clear();
                }
                if commit.sections.contains(SectionMask::TRANSCRIPT) {
                    self.scroller.update(cx, |state, cx| {
                        let count = commit.snapshot.transcript.len();
                        if epoch_changed {
                            state.reset(count, cx);
                        } else if count > old_rows {
                            state.append(count - old_rows, cx);
                        } else if count < old_rows {
                            state.splice(count..old_rows, 0, cx);
                        } else if commit_gap {
                            if count > 0 {
                                state.remeasure_items(0..count, cx);
                            }
                        } else {
                            for row in commit.dirty_rows.iter() {
                                if let Some(index) = commit
                                    .snapshot
                                    .transcript
                                    .iter()
                                    .position(|candidate| candidate.id == *row)
                                {
                                    state.remeasure_items(index..index + 1, cx);
                                }
                            }
                        }
                    });
                }
                self.last_commit_revision = commit.revision;
                let became_ready = self.snapshot.session.phase != SessionPhase::Ready
                    && commit.snapshot.session.phase == SessionPhase::Ready;
                let appearance_changed = self.snapshot.appearance != commit.snapshot.appearance;
                let theme_changed = self.snapshot.theme != commit.snapshot.theme;
                self.snapshot = commit.snapshot;
                let pending_gates = self.snapshot.pending_gates.len();
                if pending_gates != self.last_badge {
                    self.last_badge = pending_gates;
                    crate::platform::set_dock_badge(pending_gates);
                }
                if became_ready {
                    self.focus_composer_on_ready = true;
                }
                if let Some(Overlay::Form(form)) = &mut self.overlay
                    && form.choice.is_none()
                    && let FormAction::Command(id) = &form.action
                {
                    let choices = choice_values(id.clone(), &self.snapshot);
                    if !choices.is_empty() {
                        form.choice = Some(choices);
                        form.field = None;
                        form.required = false;
                    }
                }
                if appearance_changed
                    || theme_changed
                    || self.snapshot.appearance != self.last_appearance
                    || self.snapshot.theme != self.last_theme
                {
                    self.last_appearance = self.snapshot.appearance;
                    self.last_theme = self.snapshot.theme;
                    theme::apply(self.snapshot.theme, self.snapshot.appearance, None, cx);
                }
                self.sync_prompt_state(cx);
                self.refresh_typed_reader();
                cx.notify();
            }
            DesktopEvent::Voice(event) => {
                use tact_desktop_core::VoiceEvent;
                match event {
                    VoiceEvent::RecordingStarted => {
                        self.voice.listening = true;
                        self.voice.transcribing = false;
                    }
                    VoiceEvent::RecordingStopped | VoiceEvent::Transcribing => {
                        self.voice.listening = false;
                        self.voice.transcribing = true;
                    }
                    VoiceEvent::Transcript(text) => {
                        self.voice.listening = false;
                        self.voice.transcribing = false;
                        self.pending_voice_text = Some(text);
                    }
                    VoiceEvent::Error(message) => {
                        self.voice.listening = false;
                        self.voice.transcribing = false;
                        self.show_toast(
                            Toast {
                                title: self.text(TextKey::VoiceInput).into(),
                                body: message.into(),
                            },
                            false,
                            cx,
                        );
                    }
                    VoiceEvent::Cancelled => {
                        self.voice.listening = false;
                        self.voice.transcribing = false;
                    }
                }
                cx.notify();
            }
            DesktopEvent::Effect(effect) => self.handle_effect(effect, cx),
            DesktopEvent::Fatal(failure) => {
                self.show_toast(
                    Toast {
                        title: self.text(TextKey::TactStopped).into(),
                        body: failure.message.to_string().into(),
                    },
                    false,
                    cx,
                );
            }
        }
    }

    fn handle_effect(&mut self, effect: DesktopEffect, cx: &mut Context<Self>) {
        match effect {
            DesktopEffect::CopyText(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            DesktopEffect::OpenReader(request) => {
                let reference = self.pending_reader.take();
                if let Some(Overlay::Reader(reader)) = &mut self.overlay {
                    reader.title = request.title.to_string().into();
                    reader.markdown = request.markdown.to_string().into();
                    reader.reference = reference;
                    reader.artifact_view = ArtifactView::Preview;
                    reader.diff_table = None;
                    reader.decisions.clear();
                    reader.diff_epoch = 0;
                    reader.diff_applied = 0;
                } else {
                    let mut reader =
                        ReaderState::new(request.title.to_string(), request.markdown.to_string());
                    reader.reference = reference;
                    self.open_overlay(Overlay::Reader(reader));
                }
            }
            DesktopEffect::OpenPath(path) | DesktopEffect::RevealInFinder(path) => {
                cx.open_with_system(&path);
            }
            DesktopEffect::OpenExternal(url) => cx.open_url(url.as_str()),
            DesktopEffect::Notify { title, body } => {
                if self.snapshot.notifications {
                    cx.show_system_notification(gpui::SystemNotification {
                        tag: format!("tact-desktop-{}", self.snapshot.revision).into(),
                        title: title.clone().into(),
                        body: body.clone().into(),
                        actions: Vec::new(),
                    });
                }
                self.show_toast(
                    Toast {
                        title: title.into(),
                        body: body.into(),
                    },
                    true,
                    cx,
                );
            }
            DesktopEffect::SaveTranscript {
                markdown,
                suggested_name,
            } => {
                let receiver = cx.prompt_for_new_path(
                    self.snapshot
                        .project
                        .root
                        .as_deref()
                        .unwrap_or_else(|| std::path::Path::new(".")),
                    Some(&suggested_name),
                );
                let weak = cx.weak_entity();
                cx.spawn(async move |_, cx| {
                    if let Ok(Ok(Some(path))) = receiver.await {
                        let _ = std::fs::write(&path, markdown);
                        let _ = weak.update(cx, |this, cx| {
                            this.show_toast(
                                Toast {
                                    title: this.text(TextKey::TranscriptSaved).into(),
                                    body: path.display().to_string().into(),
                                },
                                false,
                                cx,
                            );
                        });
                    }
                })
                .detach();
            }
            DesktopEffect::WindowCloseRequested => cx.quit(),
        }
        cx.notify();
    }

    fn show_toast(&mut self, toast: Toast, auto_hide: bool, cx: &mut Context<Self>) {
        self.toast = Some(toast);
        self.toast_epoch = self.toast_epoch.wrapping_add(1);
        if auto_hide {
            let epoch = self.toast_epoch;
            let weak = cx.weak_entity();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(TOAST_HIDE_DELAY).await;
                let _ = weak.update(cx, |this, cx| {
                    if this.toast_epoch == epoch {
                        this.toast = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn snapshot(&self) -> Arc<DesktopSnapshot> {
        self.snapshot.clone()
    }

    #[cfg(any(test, feature = "ui-capture"))]
    pub(crate) fn set_snapshot_for_capture(
        &mut self,
        snapshot: Arc<DesktopSnapshot>,
        cx: &mut Context<Self>,
    ) {
        self.snapshot = snapshot;
        self.transcript_rows.clear();
        let count = self.snapshot.transcript.len();
        self.scroller.update(cx, |state, cx| state.reset(count, cx));
        self.sync_prompt_state(cx);
        cx.notify();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_command_center(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_center(window, cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_settings(&mut self) {
        self.open_overlay(Overlay::Settings);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_session_manager(&mut self) {
        self.open_overlay(Overlay::SessionManager);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_extensions(&mut self, tab: ExtensionsTab) {
        self.open_overlay(Overlay::Extensions(tab));
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_scroll_to(&mut self, index: usize, cx: &mut Context<Self>) {
        self.scroller
            .update(cx, |state, cx| state.scroll_to_item(index, cx));
        cx.notify();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_show_reader(
        &mut self,
        request: tact_desktop_core::ReaderRequest,
        cx: &mut Context<Self>,
    ) {
        self.handle_effect(DesktopEffect::OpenReader(request), cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_show_transcript_reader(
        &mut self,
        reference: ReaderRef,
        cx: &mut Context<Self>,
    ) {
        let request = self.transcript_reader_request(&reference);
        self.pending_reader = Some(reference);
        self.handle_effect(DesktopEffect::OpenReader(request), cx);
    }

    #[cfg(test)]
    fn open_transcript_reader_for_test(&mut self, reference: ReaderRef, cx: &mut Context<Self>) {
        let request = self.transcript_reader_request(&reference);
        self.pending_reader = Some(reference);
        self.handle_effect(DesktopEffect::OpenReader(request), cx);
    }

    /// Builds a transcript reader without going through the controller.
    ///
    /// The capture harness renders readers for seeded rows, so it needs the
    /// same title and body the controller would have produced.
    #[cfg(any(test, feature = "ui-capture"))]
    fn transcript_reader_request(&self, reference: &ReaderRef) -> tact_desktop_core::ReaderRequest {
        let (title, markdown) = match reference {
            ReaderRef::ToolOutput(row) => self
                .tool_row(*row)
                .map(|tool| {
                    (
                        tool.name.to_string(),
                        tool.output
                            .detail
                            .as_deref()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .unwrap_or_else(|| ("Reader".to_string(), String::new())),
            ReaderRef::Diff(row) => self
                .diff_row_patch(*row)
                .map(|patch| {
                    let path = self
                        .snapshot
                        .transcript
                        .iter()
                        .find_map(|item| match &item.body {
                            RowBody::Diff(diff) if item.id == *row => {
                                diff.path.as_deref().map(str::to_string)
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| "Diff".to_string());
                    (path, patch.to_string())
                })
                .unwrap_or_else(|| ("Diff".to_string(), String::new())),
            ReaderRef::Thinking(row) => self
                .snapshot
                .transcript
                .iter()
                .find_map(|item| match &item.body {
                    RowBody::Thinking(thinking) if item.id == *row => {
                        Some(("Thinking".to_string(), thinking.markdown.to_string()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| ("Thinking".to_string(), String::new())),
            _ => ("Reader".to_string(), String::new()),
        };
        tact_desktop_core::ReaderRequest {
            title: title.into(),
            markdown: markdown.into(),
        }
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_expand_thinking(&mut self, row: RowId, cx: &mut Context<Self>) {
        let session_id = self.transcript_session_id();
        self.expanded_thinking.insert((session_id, row));
        self.pending_row_updates.insert(row);
        cx.notify();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_hooks(&mut self) {
        self.open_overlay(Overlay::Hooks);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_subagent(&mut self, id: &str) {
        self.open_overlay(Overlay::Subagent(id.to_string()));
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_extensions_manager(&mut self, tab: ExtensionsTab) {
        self.open_overlay(Overlay::Extensions(tab));
        self.extension_manage = true;
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_background(&mut self, cx: &mut Context<Self>) {
        self.select_inspector(InspectorTab::Background, cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_model_picker(&mut self, id: SettingId, cx: &mut Context<Self>) {
        self.open_model_picker(id, cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_setting_form(
        &mut self,
        id: SettingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_setting_form(id, window, cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_reader(&mut self, kind: ReaderKind) {
        let title = match kind {
            ReaderKind::SystemPrompt => self.text(TextKey::SystemPrompt),
            ReaderKind::Stats => self.text(TextKey::SessionStatistics),
            ReaderKind::History => self.text(TextKey::TaskHistory),
            ReaderKind::TasksDag => self.text(TextKey::Tasks),
            ReaderKind::Balance => self.text(TextKey::Balance),
        };
        self.open_typed_reader(kind, title);
        self.refresh_typed_reader();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_open_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt
            .update(cx, |prompt, cx| prompt.set_draft("/", window, cx));
        self.refresh_slash_overlay("/", cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_reset_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_slash_overlay(cx);
        self.prompt
            .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
        cx.notify();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_attach_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.attachments.push(path);
        self.sync_prompt_attachments(cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_clear_attachments(&mut self, cx: &mut Context<Self>) {
        self.attachments.clear();
        self.sync_prompt_attachments(cx);
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_set_inspector(&mut self, tab: InspectorTab, cx: &mut Context<Self>) {
        let mut snapshot = (*self.snapshot).clone();
        snapshot.inspector.tab = tab;
        self.snapshot = Arc::new(snapshot);
        self.right_visible = true;
        cx.notify();
    }

    /// Switch the run room's page for the capture harness.
    #[cfg(any(test, feature = "ui-capture"))]
    pub(crate) fn capture_set_run_tab(&mut self, tab: RunTab, cx: &mut Context<Self>) {
        self.select_run_tab(tab, cx);
    }

    /// Turn the ledger's live filter on for the capture harness.
    #[cfg(any(test, feature = "ui-capture"))]
    pub(crate) fn capture_set_trajectory_query(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.trajectory_search
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.set_trajectory_query(query.to_string(), window, cx);
    }

    /// Select one ledger record for the capture harness.
    #[cfg(any(test, feature = "ui-capture"))]
    pub(crate) fn capture_select_trajectory_record(
        &mut self,
        id: TrajectoryRecordId,
        cx: &mut Context<Self>,
    ) {
        self.select_trajectory_record(id, cx);
    }

    /// Expand the sticky strip for a capture, so the grouped task list is
    /// reviewable rather than only its collapsed header.
    ///
    /// Capture-only: the strip's grouping is asserted through
    /// `debug_bounds` in `root::tests`, so a test build has no caller.
    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_set_sticky_tasks_collapsed(
        &mut self,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) {
        self.sticky_tasks_collapsed = collapsed;
        cx.notify();
    }

    /// Record a capture-facing detail-tab switch.
    #[cfg(any(test, feature = "ui-capture"))]
    pub(crate) fn capture_set_trajectory_tab(
        &mut self,
        tab: TrajectoryDetailTab,
        cx: &mut Context<Self>,
    ) {
        self.trajectory_tab = tab;
        cx.notify();
    }

    #[cfg(feature = "ui-capture")]
    pub(crate) fn capture_close_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        self.overlay_stack.clear();
        self.focus_restore.clear();
        self.focus_restore_pending = false;
        self.right_visible = false;
        cx.notify();
    }

    fn sync_prompt_state(&mut self, cx: &mut Context<Self>) {
        let project_root = self.snapshot.project.root.clone();
        if project_root != self.mentions_root {
            self.mentions_root = project_root;
            self.prompt_mentions = self.workspace_mentions();
        }
        let mentions = self.prompt_mentions.clone();
        let current_model = self
            .snapshot
            .composer
            .model
            .as_ref()
            .is_empty()
            .then(|| self.snapshot.runtime.model.as_deref())
            .flatten()
            .unwrap_or(self.snapshot.composer.model.as_ref());
        let models: Vec<PromptModel> = if self.snapshot.composer.models.is_empty() {
            if current_model.is_empty() {
                Vec::new()
            } else {
                vec![current_model.to_string()]
            }
        } else {
            self.snapshot
                .composer
                .models
                .iter()
                .map(ToString::to_string)
                .collect()
        }
        .into_iter()
        .map(|model| {
            PromptModel::new(model.clone(), model)
                .context_window(self.snapshot.runtime.usage.context_window.unwrap_or(0) as u64)
        })
        .collect();
        let running = matches!(
            self.snapshot.session.phase,
            SessionPhase::Running | SessionPhase::Starting | SessionPhase::Opening
        );
        let footer_status: Option<SharedString> = running.then(|| {
            SharedString::from(match self.snapshot.language {
                tact_catalog::CatalogLocale::En => "Running · Esc interrupts",
                tact_catalog::CatalogLocale::Zh => "运行中 · Esc 中断",
            })
        });
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_models(models, cx);
            if !current_model.is_empty() {
                prompt.set_selected_model(current_model.to_string(), cx);
            }
            prompt.set_progress(
                if running {
                    ProgressState::Running
                } else {
                    ProgressState::Pending
                },
                cx,
            );
            prompt.set_progress_visible(false, cx);
            prompt.set_footer_status(footer_status, cx);
            prompt.set_submit(PromptSubmit::Plane, cx);
            prompt.set_mentions(mentions, cx);
        });
        self.sync_prompt_commands(cx);
    }

    fn workspace_mentions(&self) -> Vec<PromptMention> {
        let Some(root) = self.snapshot.project.root.as_ref() else {
            return Vec::new();
        };
        fn walk(root: &std::path::Path, dir: &std::path::Path, output: &mut Vec<PromptMention>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    if matches!(name.as_ref(), ".git" | "target" | ".tact" | "node_modules") {
                        continue;
                    }
                    walk(root, &path, output);
                } else if let Ok(relative) = path.strip_prefix(root) {
                    output.push(PromptMention::new(
                        path.display().to_string(),
                        relative.display().to_string(),
                    ));
                }
            }
        }
        let mut output = Vec::new();
        walk(root, root, &mut output);
        output
    }

    fn refresh_workspace_mentions(&mut self, cx: &mut Context<Self>) {
        self.prompt_mentions = self.workspace_mentions();
        let mentions = self.prompt_mentions.clone();
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_mentions(mentions, cx);
        });
    }

    fn sync_prompt_commands(&mut self, cx: &mut Context<Self>) {
        let commands = Vec::<PromptCommand>::new();
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_commands(commands, cx);
        });
    }

    fn on_prompt_event(
        &mut self,
        prompt: &Entity<PromptBar>,
        event: &PromptBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PromptBarEvent::Submit { submission, .. } => {
                let text = submission.text().trim().to_string();
                if text.starts_with('/') {
                    self.execute_slash_text(&text, window, cx);
                } else if !text.is_empty() {
                    self.dispatch(DesktopIntent::Submit(Submission {
                        text,
                        attachments: self.attachments(),
                        model: submission.model_id().map(ToString::to_string),
                        permission_mode: None,
                    }));
                }
                prompt.update(cx, |prompt, cx| prompt.set_draft("", window, cx));
                self.history_cursor = None;
                self.slash_overlay = None;
                self.slash_dismissed_for = None;
            }
            PromptBarEvent::CancelRequested { .. } => {
                self.dispatch(DesktopIntent::CancelRun);
            }
            PromptBarEvent::CommandSelected { command_id, .. } => {
                if let Some(command) = find_command(command_id.as_ref()) {
                    if !command.children.is_empty() {
                        let path = format!("/{} ", command.path.join(" "));
                        prompt.update(cx, |prompt, cx| prompt.set_draft(path, window, cx));
                        self.sync_nested_commands(&command.id, window, cx);
                        prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
                    } else if command.args.is_empty() {
                        prompt.update(cx, |prompt, cx| prompt.set_draft("", window, cx));
                        self.execute_command(
                            command.id.clone(),
                            CommandArguments::default(),
                            window,
                            cx,
                        );
                    } else {
                        self.remember_focus(window, cx);
                        self.open_form(command, window, cx);
                    }
                }
            }
            PromptBarEvent::DraftChanged { draft, .. } => {
                let draft = draft.to_string();
                if draft.contains('@') {
                    self.refresh_workspace_mentions(cx);
                }
                self.refresh_slash_overlay(&draft, cx);
            }
            PromptBarEvent::AttachRequested { .. } => self.pick_attachments(cx),
            PromptBarEvent::ImagePasted { path, .. } => {
                if !self.attachments.contains(path) {
                    self.attachments.push(path.clone());
                    self.sync_prompt_attachments(cx);
                }
            }
            PromptBarEvent::AttachmentRemoved { attachment_id, .. } => {
                self.attachments
                    .retain(|path| path.display().to_string() != attachment_id.as_ref());
                self.sync_prompt_attachments(cx);
            }
            PromptBarEvent::MentionSelected { .. } => {}
            PromptBarEvent::ModelChanged { model_id, .. } => {
                self.dispatch(DesktopIntent::SetSetting {
                    id: SettingId::Model,
                    value: SettingValue::Text(model_id.to_string()),
                });
            }
            PromptBarEvent::EnhanceRequested { .. } => {
                self.show_toast(
                    Toast {
                        title: self.text(TextKey::PromptEnhancement).into(),
                        body: self.text(TextKey::PromptEnhancementUnavailable).into(),
                    },
                    false,
                    cx,
                );
            }
        }
    }

    fn submit_composer_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).draft(cx).trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') {
            self.execute_slash_text(&text, window, cx);
        } else {
            self.history_cursor = None;
            self.dispatch(DesktopIntent::Submit(Submission {
                text,
                attachments: self.attachments(),
                model: None,
                permission_mode: None,
            }));
        }
        self.prompt
            .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
        self.slash_overlay = None;
        self.slash_dismissed_for = None;
    }

    fn sync_nested_commands(
        &mut self,
        parent: &CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = tact_catalog::builtins()
            .children(parent)
            .map(|entry| {
                PromptCommand::new(
                    command_key(&entry.id),
                    entry.path.last().cloned().unwrap_or_default(),
                )
                .description(entry.description.get(self.snapshot.language).to_string())
            })
            .collect::<Vec<_>>();
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_commands(commands, cx);
        });
        let _ = window;
    }

    fn refresh_slash_overlay(&mut self, draft: &str, cx: &mut Context<Self>) {
        if !draft.starts_with('/') || draft.contains(['\n', '\r']) {
            self.slash_dismissed_for = None;
            if self.slash_overlay.take().is_some() {
                cx.notify();
            }
            return;
        }
        if self.slash_dismissed_for.as_deref() == Some(draft) {
            if self.slash_overlay.take().is_some() {
                cx.notify();
            }
            return;
        }
        let availability = self.command_availability();
        let skills = self
            .snapshot
            .extensions
            .snapshot
            .skills
            .iter()
            .map(|skill| {
                let detail = if skill.enabled {
                    skill.description.clone()
                } else {
                    format!(
                        "{} · {}",
                        skill.description,
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => "disabled",
                            tact_catalog::CatalogLocale::Zh => "已禁用",
                        }
                    )
                };
                (skill.name.clone(), detail)
            })
            .collect::<Vec<_>>();
        let mut suggestions = crate::ui::slash_overlay::command_candidates(
            draft,
            self.snapshot.language,
            availability,
            &skills,
        )
        .into_iter()
        .map(|candidate| SlashSuggestion {
            title: candidate.title,
            detail: candidate.detail,
            insertion: candidate.insertion,
            has_children: candidate.has_children,
        })
        .collect::<Vec<_>>();
        if suggestions.is_empty()
            && let Some((descriptor, args)) = crate::ui::slash_overlay::resolve(draft)
            && descriptor.children.is_empty()
            && !descriptor.args.is_empty()
            && args.tokens.len() <= 1
            && is_enabled(descriptor, availability)
        {
            let trailing_space = draft.ends_with(char::is_whitespace);
            let fragment = if trailing_space {
                ""
            } else {
                args.tokens.first().map(String::as_str).unwrap_or("")
            };
            let path = descriptor.path.join(" ");
            for (label, value) in choice_values(descriptor.id.clone(), &self.snapshot) {
                let score = tact_catalog::fuzzy_score(fragment, &label)
                    .or_else(|| tact_catalog::fuzzy_score(fragment, &value));
                if score.is_none() {
                    continue;
                }
                suggestions.push(SlashSuggestion {
                    title: label,
                    detail: path.clone(),
                    insertion: format!("/{path} {value}"),
                    has_children: false,
                });
            }
        }
        let previous = self
            .slash_overlay
            .as_ref()
            .and_then(|overlay| overlay.suggestions.get(overlay.active))
            .map(|suggestion| suggestion.insertion.as_str());
        let active = previous
            .and_then(|previous| {
                suggestions
                    .iter()
                    .position(|suggestion| suggestion.insertion == previous)
            })
            .unwrap_or(0);
        self.slash_overlay = (!suggestions.is_empty()).then_some(SlashOverlayState {
            suggestions,
            active,
        });
        cx.notify();
    }

    fn move_slash_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(overlay) = self.slash_overlay.as_mut() else {
            return;
        };
        if overlay.suggestions.is_empty() {
            return;
        }
        let len = overlay.suggestions.len();
        overlay.active = if delta < 0 {
            overlay.active.checked_sub(1).unwrap_or(len - 1)
        } else {
            (overlay.active + 1) % len
        };
        cx.notify();
    }

    fn accept_slash_suggestion(
        &mut self,
        execute: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(suggestion) = self
            .slash_overlay
            .as_ref()
            .and_then(|overlay| overlay.suggestions.get(overlay.active))
            .cloned()
        else {
            if execute {
                let draft = self.prompt.read(cx).draft(cx).to_string();
                self.slash_overlay = None;
                self.execute_slash_text(&draft, window, cx);
            }
            return;
        };
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_draft_at_end(suggestion.insertion.clone(), window, cx);
            prompt.focus(window, cx);
        });
        self.refresh_slash_overlay(&suggestion.insertion, cx);
        let executable_parent = suggestion.insertion.trim() == "/goal list";
        if execute && (!suggestion.has_children || executable_parent) {
            self.slash_overlay = None;
            if executable_parent {
                self.execute_command(
                    CommandId::BuiltinPath {
                        root: BuiltinCommand::Goal,
                        path: vec!["goal", "list"],
                    },
                    CommandArguments::default(),
                    window,
                    cx,
                );
            } else {
                self.execute_slash_text(&suggestion.insertion, window, cx);
            }
        }
    }

    fn dismiss_slash_overlay(&mut self, cx: &mut Context<Self>) {
        if self.slash_overlay.is_none() {
            return;
        }
        self.slash_dismissed_for = Some(self.prompt.read(cx).draft(cx).to_string());
        self.slash_overlay = None;
        cx.notify();
    }

    fn handle_intercepted_key(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.overlay.is_none() && self.slash_overlay.is_some() {
            match key {
                "down" => self.move_slash_selection(1, cx),
                "up" => self.move_slash_selection(-1, cx),
                "tab" if shift => self.move_slash_selection(-1, cx),
                "tab" => self.accept_slash_suggestion(false, window, cx),
                "enter" if !shift => self.accept_slash_suggestion(true, window, cx),
                "escape" => {
                    self.dismiss_slash_overlay(cx);
                    self.prompt
                        .update(cx, |prompt, cx| prompt.focus(window, cx));
                }
                _ => return false,
            }
            return true;
        }
        if self.overlay.is_none() && self.slash_overlay.is_none() {
            let history_len = self.snapshot.composer.history.len();
            let draft_empty = self.prompt.read(cx).draft(cx).is_empty();
            match key {
                "up" if history_len > 0 && (self.history_cursor.is_some() || draft_empty) => {
                    let next = self
                        .history_cursor
                        .map(|cursor| (cursor + 1).min(history_len - 1))
                        .unwrap_or(0);
                    self.history_cursor = Some(next);
                    let entry = self.snapshot.composer.history[next].to_string();
                    self.prompt
                        .update(cx, |prompt, cx| prompt.set_draft(entry, window, cx));
                    cx.notify();
                    return true;
                }
                "down" if self.history_cursor.is_some() => {
                    let cursor = self.history_cursor.unwrap_or(0);
                    if cursor == 0 {
                        self.history_cursor = None;
                        self.prompt
                            .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
                    } else {
                        self.history_cursor = Some(cursor - 1);
                        let entry = self.snapshot.composer.history[cursor - 1].to_string();
                        self.prompt
                            .update(cx, |prompt, cx| prompt.set_draft(entry, window, cx));
                    }
                    cx.notify();
                    return true;
                }
                "escape" if self.history_cursor.is_some() => {
                    self.history_cursor = None;
                    self.prompt
                        .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }
        if matches!(self.overlay, Some(Overlay::ModelPicker(_))) {
            match key {
                "down" => {
                    let count = self.model_picker_len();
                    if count > 0 {
                        self.model_picker_cursor = (self.model_picker_cursor + 1) % count;
                        self.reveal_model_cursor(cx);
                        cx.notify();
                    }
                    return true;
                }
                "up" => {
                    let count = self.model_picker_len();
                    if count > 0 {
                        self.model_picker_cursor =
                            self.model_picker_cursor.checked_sub(1).unwrap_or(count - 1);
                        self.reveal_model_cursor(cx);
                        cx.notify();
                    }
                    return true;
                }
                "enter" if !shift => {
                    self.activate_model_picker(window, cx);
                    return true;
                }
                "escape" => {
                    self.close_overlay();
                    self.restore_focus(window, cx);
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn execute_slash_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((descriptor, args)) = crate::ui::slash_overlay::resolve(text) else {
            let body = text.trim().strip_prefix('/').unwrap_or_default();
            let mut tokens = body.split_whitespace();
            let Some(name) = tokens.next() else {
                return;
            };
            if self
                .snapshot
                .extensions
                .snapshot
                .skills
                .iter()
                .any(|skill| skill.name == name)
            {
                self.execute_command(
                    CommandId::DynamicSkill(name.to_string()),
                    CommandArguments {
                        tokens: tokens.map(ToOwned::to_owned).collect(),
                    },
                    window,
                    cx,
                );
            }
            return;
        };
        let tokens = args.tokens;
        if descriptor.children.is_empty() {
            let required_args = descriptor
                .args
                .iter()
                .filter(|argument| argument.required)
                .count();
            if tokens.len() < required_args {
                if matches!(
                    &descriptor.id,
                    CommandId::BuiltinPath {
                        root: BuiltinCommand::Goal,
                        path,
                    } if matches!(path.as_slice(), ["goal", "steer" | "accept"])
                ) {
                    self.execute_command(
                        descriptor.id.clone(),
                        CommandArguments { tokens },
                        window,
                        cx,
                    );
                } else {
                    self.remember_focus(window, cx);
                    self.open_form(descriptor, window, cx);
                }
                return;
            }
            self.execute_command(
                descriptor.id.clone(),
                CommandArguments { tokens },
                window,
                cx,
            );
            self.prompt
                .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
            return;
        }
        if tokens.is_empty() {
            self.remember_focus(window, cx);
            self.open_form(descriptor, window, cx);
            return;
        }
        let mut skill_tokens = descriptor.path.iter().skip(1).cloned().collect::<Vec<_>>();
        skill_tokens.extend(tokens);
        if descriptor.id == CommandId::Builtin(BuiltinCommand::Skill) && !skill_tokens.is_empty() {
            self.execute_command(
                CommandId::Builtin(BuiltinCommand::Skill),
                CommandArguments {
                    tokens: skill_tokens,
                },
                window,
                cx,
            );
            self.prompt
                .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
            return;
        }
        self.show_toast(
            Toast {
                title: self.text(TextKey::UnknownCommand).into(),
                body: text.to_string().into(),
            },
            false,
            cx,
        );
    }

    fn execute_command(
        &mut self,
        id: CommandId,
        args: CommandArguments,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let availability = self.command_availability();
        if let Some(descriptor) = tact_catalog::builtins().get(&id)
            && !is_enabled(descriptor, availability)
        {
            self.show_toast(
                Toast {
                    title: self.text(TextKey::CommandUnavailable).into(),
                    body: descriptor.description.get(self.snapshot.language).into(),
                },
                false,
                cx,
            );
            return;
        }
        self.remember_focus(window, cx);
        self.remember_command(id.clone());
        match id.clone() {
            CommandId::Builtin(BuiltinCommand::ViewSystemPrompt) => {
                self.open_typed_reader(ReaderKind::SystemPrompt, self.text(TextKey::SystemPrompt));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Builtin(BuiltinCommand::Stats) => {
                self.open_typed_reader(ReaderKind::Stats, self.text(TextKey::SessionStatistics));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Builtin(BuiltinCommand::History) => {
                self.open_typed_reader(ReaderKind::History, self.text(TextKey::TaskHistory));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Builtin(BuiltinCommand::TasksDag) => {
                self.open_typed_reader(ReaderKind::TasksDag, self.text(TextKey::Tasks));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Builtin(BuiltinCommand::Balance) => {
                self.open_typed_reader(ReaderKind::Balance, self.text(TextKey::Balance));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Builtin(BuiltinCommand::ModelSubagent) => {
                if let Some(model) = args.tokens.first() {
                    self.dispatch(DesktopIntent::SetSetting {
                        id: SettingId::SubagentModel,
                        value: SettingValue::Text(model.clone()),
                    });
                } else {
                    self.open_model_picker(SettingId::SubagentModel, cx);
                }
            }
            CommandId::Builtin(BuiltinCommand::Model) => {
                if let Some(model) = args.tokens.first() {
                    self.dispatch(DesktopIntent::SetSetting {
                        id: SettingId::Model,
                        value: SettingValue::Text(model.clone()),
                    });
                } else {
                    self.open_model_picker(SettingId::Model, cx);
                }
            }
            CommandId::Builtin(BuiltinCommand::Permission) => {
                if let Some(mode) = args.tokens.first() {
                    self.dispatch(DesktopIntent::SetSetting {
                        id: SettingId::PermissionMode,
                        value: SettingValue::Choice(mode.clone()),
                    });
                } else {
                    self.open_setting_form(SettingId::PermissionMode, window, cx);
                }
            }
            CommandId::Builtin(
                command @ (BuiltinCommand::Mcp
                | BuiltinCommand::Skill
                | BuiltinCommand::Plugin
                | BuiltinCommand::Hooks),
            ) => {
                if matches!(command, BuiltinCommand::Hooks) {
                    self.open_overlay(Overlay::Hooks);
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: id.clone(),
                        args: args.clone(),
                    });
                    self.refresh_hooks_surface();
                    cx.notify();
                    return;
                } else {
                    self.open_overlay(Overlay::Extensions(match command {
                        BuiltinCommand::Mcp => ExtensionsTab::Mcp,
                        BuiltinCommand::Skill => ExtensionsTab::Skills,
                        BuiltinCommand::Plugin => ExtensionsTab::Plugins,
                        _ => ExtensionsTab::Mcp,
                    }));
                }
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Setting(id) => {
                if matches!(id, SettingId::Model | SettingId::SubagentModel) {
                    self.open_model_picker(id, cx);
                } else {
                    self.open_setting_form(id, window, cx);
                }
            }
            CommandId::Composer(action) => match action {
                tact_catalog::ComposerAction::MentionFile => self.pick_files(cx),
                tact_catalog::ComposerAction::AttachImage => self.pick_attachments(cx),
                tact_catalog::ComposerAction::Voice => {
                    self.show_toast(
                        Toast {
                            title: self.text(TextKey::VoiceInput).into(),
                            body: self.text(TextKey::VoiceInputUnavailable).into(),
                        },
                        false,
                        cx,
                    );
                }
                tact_catalog::ComposerAction::Queue => {
                    let draft = self.prompt.read(cx).draft(cx).to_string();
                    if !draft.trim().is_empty() {
                        self.dispatch(DesktopIntent::Queue(Submission {
                            text: draft.trim().to_string(),
                            attachments: self.attachments(),
                            model: None,
                            permission_mode: None,
                        }));
                        self.prompt
                            .update(cx, |prompt, cx| prompt.set_draft("", window, cx));
                    }
                }
                tact_catalog::ComposerAction::Stop => self.dispatch(DesktopIntent::CancelRun),
                tact_catalog::ComposerAction::Compact => {
                    self.dispatch(DesktopIntent::InvokeCommand { id, args });
                }
                tact_catalog::ComposerAction::SessionActions => {
                    self.remember_focus(window, cx);
                    self.session_project_filter = self.snapshot.project.id.clone();
                    self.session_search.update(cx, |input, cx| {
                        input.focus(window, cx);
                    });
                    self.open_overlay(Overlay::SessionManager);
                    self.overlay_focus_pending = false;
                }
            },
            CommandId::Cli(tact_catalog::CliEntry::ListSessions) => {
                self.dispatch(DesktopIntent::SetSessionFilter(ArchiveFilter::All));
                self.left_visible = true;
            }
            CommandId::Cli(tact_catalog::CliEntry::ResumeLast) => {
                self.dispatch(DesktopIntent::ResumeLast)
            }
            CommandId::Cli(
                tact_catalog::CliEntry::PluginList
                | tact_catalog::CliEntry::PluginInstall
                | tact_catalog::CliEntry::PluginUninstall
                | tact_catalog::CliEntry::PluginUpdate
                | tact_catalog::CliEntry::PluginReload
                | tact_catalog::CliEntry::MarketplaceAdd
                | tact_catalog::CliEntry::MarketplaceList
                | tact_catalog::CliEntry::MarketplaceUpdate
                | tact_catalog::CliEntry::MarketplaceRemove,
            ) => {
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Plugins));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Cli(
                tact_catalog::CliEntry::McpList
                | tact_catalog::CliEntry::McpGet
                | tact_catalog::CliEntry::McpRemove
                | tact_catalog::CliEntry::McpLogin
                | tact_catalog::CliEntry::McpLogout,
            ) => {
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Cli(tact_catalog::CliEntry::McpAdd) => {
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
                if args.tokens.len() > 1 {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::Cli(tact_catalog::CliEntry::McpAdd),
                        args,
                    });
                } else {
                    let name = args.tokens.first().cloned().unwrap_or_default();
                    self.set_form(
                        FormState {
                            title: self.text(TextKey::AddMcpServer).into(),
                            action: FormAction::McpAddTransport(name),
                            field: Some(self.text(TextKey::RemoteUrl).into()),
                            required: true,
                            choice: None,
                        },
                        window,
                        cx,
                    );
                }
            }
            CommandId::Cli(
                tact_catalog::CliEntry::HooksList
                | tact_catalog::CliEntry::HooksTrust
                | tact_catalog::CliEntry::HooksForget,
            ) => {
                self.open_overlay(Overlay::Hooks);
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
                self.refresh_hooks_surface();
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Goal,
                path,
            } if matches!(path.as_slice(), ["goal", "steer" | "accept"])
                && args.tokens.len() < 2 =>
            {
                if let Some(goal_id) = args.tokens.first().cloned() {
                    self.set_form(
                        FormState {
                            title: path.join(" ").into(),
                            action: if path.last() == Some(&"accept") {
                                FormAction::AcceptGoal(goal_id)
                            } else {
                                FormAction::SteerGoal(goal_id)
                            },
                            field: Some(argument_label("text", self.snapshot.language).into()),
                            required: true,
                            choice: None,
                        },
                        window,
                        cx,
                    );
                } else {
                    self.open_form(
                        tact_catalog::builtins()
                            .get(&CommandId::BuiltinPath {
                                root: BuiltinCommand::Goal,
                                path,
                            })
                            .expect("goal descriptor"),
                        window,
                        cx,
                    );
                }
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Skill,
                path,
            } => {
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Skills));
                if matches!(path.last().copied(), Some("enable" | "disable"))
                    && let Some(name) = args.tokens.first()
                {
                    self.dispatch(DesktopIntent::SetSkillEnabled {
                        name: name.clone(),
                        enabled: path.last() == Some(&"enable"),
                    });
                } else {
                    self.dispatch(DesktopIntent::InvokeCommand { id, args });
                }
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Plugin,
                ..
            } => {
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Plugins));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Mcp,
                ..
            } => {
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Hooks,
                ref path,
            } => {
                self.open_overlay(Overlay::Hooks);
                self.dispatch(DesktopIntent::InvokeCommand {
                    id: if matches!(path.as_slice(), ["hooks", "trust" | "forget"]) {
                        CommandId::BuiltinPath {
                            root: BuiltinCommand::Hooks,
                            path: vec!["hooks", "list"],
                        }
                    } else {
                        id
                    },
                    args,
                });
            }
            CommandId::Builtin(BuiltinCommand::Goal) => {
                self.select_inspector(InspectorTab::Goals, cx);
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Goal,
                ..
            } => {
                self.select_inspector(InspectorTab::Goals, cx);
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::Cli(_)
            | CommandId::BuiltinPath { .. }
            | CommandId::DynamicSkill(_)
            | CommandId::Builtin(_) => {
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
        }
        cx.notify();
    }

    fn refresh_hooks_surface(&mut self) {
        self.dispatch(DesktopIntent::InvokeCommand {
            id: CommandId::BuiltinPath {
                root: BuiltinCommand::Hooks,
                path: vec!["hooks", "list"],
            },
            args: CommandArguments::default(),
        });
    }

    fn open_model_picker(&mut self, id: SettingId, cx: &mut Context<Self>) {
        let (models, selected) = if id == SettingId::SubagentModel {
            (
                self.snapshot.composer.subagent_models.as_ref(),
                self.snapshot.composer.subagent_model.as_ref(),
            )
        } else {
            (
                self.snapshot.composer.models.as_ref(),
                self.snapshot.composer.model.as_ref(),
            )
        };
        // Each opening starts from the unfiltered list, at the selected model.
        self.model_picker_query.clear();
        self.model_picker_scroll.reset();
        self.model_picker_cursor = models
            .iter()
            .position(|model| model.as_ref() == selected)
            .unwrap_or(0);
        self.open_overlay(Overlay::ModelPicker(id));
        // The selected model can sit past the first screenful; open with it in
        // view rather than at the top of an unfiltered index.
        self.reveal_model_cursor(cx);
        cx.notify();
    }

    fn model_picker_len(&self) -> usize {
        self.model_picker_models().len()
    }

    /// The models the open picker can offer right now, in configuration order.
    fn model_picker_models(&self) -> Vec<Arc<str>> {
        let models: &[Arc<str>] = match self.overlay {
            Some(Overlay::ModelPicker(SettingId::SubagentModel)) => {
                &self.snapshot.composer.subagent_models
            }
            Some(Overlay::ModelPicker(_)) => &self.snapshot.composer.models,
            _ => return Vec::new(),
        };
        let query = self.model_picker_query.trim().to_lowercase();
        models
            .iter()
            .filter(|model| query.is_empty() || model.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    /// Keeps the keyboard cursor on screen after it moves.
    ///
    /// Rows are a fixed height, so the visible band is a row window rather
    /// than a pixel scroll. Wheel and keyboard share the same top row.
    fn reveal_model_cursor(&self, cx: &mut Context<Self>) {
        if self.model_picker_scroll.scroll_to(
            self.model_picker_cursor,
            self.model_picker_len(),
            MODEL_PICKER_VISIBLE_ROWS,
        ) {
            cx.notify();
        }
    }

    fn activate_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::ModelPicker(target)) = self.overlay.clone() else {
            return;
        };
        let Some(value) = self
            .model_picker_models()
            .get(self.model_picker_cursor)
            .map(ToString::to_string)
        else {
            return;
        };
        self.dispatch(DesktopIntent::SetSetting {
            id: target,
            value: SettingValue::Text(value.clone()),
        });
        if target == SettingId::Model {
            self.prompt.update(cx, |prompt, cx| {
                prompt.set_selected_model(value, cx);
            });
        }
        self.close_overlay();
        self.restore_focus(window, cx);
        cx.notify();
    }

    fn open_typed_reader(&mut self, kind: ReaderKind, title: &str) {
        let mut reader = ReaderState::new(title, self.text(TextKey::Loading));
        reader.typed = Some(kind);
        self.open_overlay(Overlay::Reader(reader));
    }

    /// Opens a transcript reader and remembers which row asked for it, so the
    /// reader can mount the typed surface for that row's content.
    fn open_transcript_reader(&mut self, reference: ReaderRef, cx: &mut Context<Self>) {
        self.pending_reader = Some(reference.clone());
        self.dispatch(DesktopIntent::OpenReader(reference));
        cx.notify();
    }

    /// Open the Chat page's reader for one transcript row.
    ///
    /// The Trajectory detail panel is a summary; the row's own reader is where
    /// the complete body lives, so "open" from the ledger lands on the same
    /// surface the row itself opens and returns the reader to the chat page
    /// where the row is addressable.
    fn open_transcript_row_reader(&mut self, row: RowId, cx: &mut Context<Self>) {
        self.select_run_tab(RunTab::Chat, cx);
        self.open_transcript_reader(ReaderRef::ToolOutput(row), cx);
    }

    fn refresh_typed_reader(&mut self) {
        let Some(Overlay::Reader(reader)) = &mut self.overlay else {
            return;
        };
        let Some(kind) = reader.typed else {
            return;
        };
        let key = format!(
            "{kind:?}:{}:{:?}",
            self.snapshot.revision,
            self.snapshot.surface.as_ref().map(surface_signature)
        );
        if self.last_surface_revision.as_deref() == Some(&key) {
            return;
        }
        self.last_surface_revision = Some(key);
        if kind == ReaderKind::TasksDag {
            let lines = crate::ui::task_dag::dependency_lines(
                &self.snapshot.inspector.tasks,
                self.snapshot.language,
            );
            reader.markdown = if lines.is_empty() {
                match self.snapshot.language {
                    tact_catalog::CatalogLocale::En => "No tasks yet.",
                    tact_catalog::CatalogLocale::Zh => "还没有任务。",
                }
                .into()
            } else {
                lines.join("\n").into()
            };
            return;
        }
        if kind == ReaderKind::Balance {
            reader.markdown = crate::ui::balance::balance_markdown(
                self.snapshot.account.as_ref(),
                self.snapshot.language,
            )
            .into();
            return;
        }
        if let Some(surface) = &self.snapshot.surface
            && let Some(markdown) = surface_reader_markdown(kind, surface, self.snapshot.language)
        {
            reader.markdown = markdown.into();
        }
    }

    fn open_form(
        &mut self,
        descriptor: &CommandDescriptor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(argument) = descriptor.args.first() {
            match argument.value {
                ArgumentValue::Choice(ChoiceSource::McpServers) => {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::BuiltinPath {
                            root: BuiltinCommand::Mcp,
                            path: vec!["mcp", "list"],
                        },
                        args: CommandArguments::default(),
                    });
                }
                ArgumentValue::Choice(ChoiceSource::HookSources) => {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::BuiltinPath {
                            root: BuiltinCommand::Hooks,
                            path: vec!["hooks", "list"],
                        },
                        args: CommandArguments::default(),
                    });
                }
                _ => {}
            }
        }
        let form = match descriptor.args.first() {
            Some(argument) => match argument.value {
                ArgumentValue::FreeText => FormState {
                    title: descriptor.path.join(" ").into(),
                    action: FormAction::Command(descriptor.id.clone()),
                    field: Some(argument_label(argument.name, self.snapshot.language).to_string()),
                    required: argument.required,
                    choice: None,
                },
                ArgumentValue::Choice(_) => {
                    let choices = choice_values(descriptor.id.clone(), &self.snapshot);
                    FormState {
                        title: descriptor.path.join(" ").into(),
                        action: FormAction::Command(descriptor.id.clone()),
                        field: choices.is_empty().then(|| {
                            argument_label(argument.name, self.snapshot.language).to_string()
                        }),
                        required: choices.is_empty() && argument.required,
                        choice: (!choices.is_empty()).then_some(choices),
                    }
                }
            },
            None => FormState {
                title: descriptor.path.join(" ").into(),
                action: FormAction::Command(descriptor.id.clone()),
                field: None,
                required: false,
                choice: None,
            },
        };
        self.set_form(form, window, cx);
    }

    fn open_setting_form(&mut self, id: SettingId, window: &mut Window, cx: &mut Context<Self>) {
        let label = tact_catalog::builtins()
            .get(&CommandId::Setting(id))
            .expect("every setting has a catalog descriptor")
            .label
            .get(self.snapshot.language)
            .to_string();
        let is_text = matches!(
            tact_desktop_core::form_for(&CommandId::Setting(id)),
            CommandForm::Text { .. }
        );
        let form = match tact_desktop_core::form_for(&CommandId::Setting(id)) {
            CommandForm::Text { required, .. } => FormState {
                title: label.clone().into(),
                action: FormAction::Command(CommandId::Setting(id)),
                field: Some(label),
                required,
                choice: None,
            },
            CommandForm::Choice { .. } => FormState {
                title: label.into(),
                action: FormAction::Command(CommandId::Setting(id)),
                field: None,
                required: false,
                choice: Some(setting_choices(id, self.snapshot.language)),
            },
            CommandForm::None => FormState {
                title: label.into(),
                action: FormAction::Command(CommandId::Setting(id)),
                field: None,
                required: false,
                choice: None,
            },
        };
        self.set_form(form, window, cx);
        if is_text && id != SettingId::ApiKey {
            let value = setting_display_value(id, &self.snapshot);
            if value != "—" {
                self.form_input
                    .update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
    }

    fn set_form(&mut self, form: FormState, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Some(Overlay::Form(_))) {
            self.overlay = Some(Overlay::Form(form));
        } else {
            self.open_overlay(Overlay::Form(form));
        }
        self.criteria_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.form_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.overlay_focus_pending = false;
        cx.notify();
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Form(form)) = &self.overlay else {
            return;
        };
        let action = form.action.clone();
        let text = self.form_input.read(cx).value().to_string();
        let criteria = self.criteria_input.read(cx).value().to_string();
        if let FormAction::McpAddTransport(name) = &action
            && !text.trim().is_empty()
        {
            self.set_form(
                FormState {
                    title: self.text(TextKey::AddMcpServer).into(),
                    action: FormAction::McpAddChoose(name.clone()),
                    field: None,
                    required: false,
                    choice: Some(vec![
                        (self.text(TextKey::RemoteUrl).into(), "url".into()),
                        (self.text(TextKey::LocalCommand).into(), "command".into()),
                    ]),
                },
                window,
                cx,
            );
            return;
        }
        self.close_overlay();
        self.restore_focus(window, cx);
        match action {
            FormAction::RenameSession(session_id) => {
                if !text.trim().is_empty() {
                    self.dispatch(DesktopIntent::RenameSession {
                        session_id,
                        title: text.trim().to_string(),
                    });
                }
            }
            FormAction::CreateGoal => {
                if !text.trim().is_empty() {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::Builtin(BuiltinCommand::Goal),
                        args: goal_create_arguments(text.trim(), &criteria),
                    });
                }
            }
            FormAction::SteerGoal(goal_id) => {
                if !text.trim().is_empty() {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::BuiltinPath {
                            root: BuiltinCommand::Goal,
                            path: vec!["goal", "steer"],
                        },
                        args: CommandArguments {
                            tokens: vec![goal_id, text.trim().into()],
                        },
                    });
                }
            }
            FormAction::AcceptGoal(goal_id) => {
                if !text.trim().is_empty() {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::BuiltinPath {
                            root: BuiltinCommand::Goal,
                            path: vec!["goal", "accept"],
                        },
                        args: CommandArguments {
                            tokens: vec![goal_id, text.trim().into()],
                        },
                    });
                }
            }
            FormAction::McpAddUrl(name) => {
                if !text.trim().is_empty() {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::Cli(tact_catalog::CliEntry::McpAdd),
                        args: CommandArguments {
                            tokens: vec![name, "--url".into(), text.trim().into()],
                        },
                    });
                }
            }
            FormAction::McpAddTransport(name) => {
                if !text.trim().is_empty() {
                    self.set_form(
                        FormState {
                            title: self.text(TextKey::AddMcpServer).into(),
                            action: FormAction::McpAddChoose(name),
                            field: None,
                            required: false,
                            choice: Some(vec![
                                (self.text(TextKey::RemoteUrl).into(), "url".into()),
                                (self.text(TextKey::LocalCommand).into(), "command".into()),
                            ]),
                        },
                        window,
                        cx,
                    );
                }
            }
            FormAction::McpAddCommand(name) => {
                if !text.trim().is_empty() {
                    self.dispatch(DesktopIntent::InvokeCommand {
                        id: CommandId::Cli(tact_catalog::CliEntry::McpAdd),
                        args: CommandArguments {
                            tokens: vec![name, "--command".into(), text.trim().into()],
                        },
                    });
                }
            }
            FormAction::McpPrompt { server, name } => {
                let mut tokens = vec![server, name];
                tokens.extend(text.split_whitespace().map(ToOwned::to_owned));
                self.execute_command(
                    CommandId::BuiltinPath {
                        root: BuiltinCommand::Mcp,
                        path: vec!["mcp", "prompt"],
                    },
                    CommandArguments { tokens },
                    window,
                    cx,
                );
            }
            FormAction::RemoveProject(_) => {}
            FormAction::ConfirmCancelRun => {}
            FormAction::McpAddChoose(_) => {}
            FormAction::Command(id) => {
                let value = match id {
                    CommandId::Setting(setting) => setting_value(setting, &text),
                    _ => FormValue::Args(CommandArguments {
                        tokens: text.split_whitespace().map(ToOwned::to_owned).collect(),
                    }),
                };
                match value {
                    FormValue::Args(args) => self.execute_command(id, args, window, cx),
                    FormValue::Setting(id, value) => {
                        self.dispatch(DesktopIntent::SetSetting { id, value });
                        self.sync_prompt_state(cx);
                    }
                }
            }
        }
        self.toast = None;
    }

    fn choose_form_option(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Form(form)) = &self.overlay else {
            return;
        };
        let action = form.action.clone();
        if let FormAction::McpAddChoose(name) = &action {
            let (next_action, field) = if value == "command" {
                (
                    FormAction::McpAddCommand(name.clone()),
                    TextKey::LocalCommand,
                )
            } else {
                (FormAction::McpAddUrl(name.clone()), TextKey::RemoteUrl)
            };
            self.set_form(
                FormState {
                    title: self.text(TextKey::AddMcpServer).into(),
                    action: next_action,
                    field: Some(self.text(field).into()),
                    required: true,
                    choice: None,
                },
                window,
                cx,
            );
            return;
        }
        self.close_overlay();
        self.restore_focus(window, cx);
        match action {
            FormAction::RenameSession(session_id) => {
                self.dispatch(DesktopIntent::RenameSession {
                    session_id,
                    title: value,
                });
            }
            FormAction::CreateGoal => {
                self.dispatch(DesktopIntent::InvokeCommand {
                    id: CommandId::Builtin(BuiltinCommand::Goal),
                    args: goal_create_arguments(&value, ""),
                });
            }
            FormAction::SteerGoal(goal_id) => {
                self.dispatch(DesktopIntent::InvokeCommand {
                    id: CommandId::BuiltinPath {
                        root: BuiltinCommand::Goal,
                        path: vec!["goal", "steer"],
                    },
                    args: CommandArguments {
                        tokens: vec![goal_id, value],
                    },
                });
            }
            FormAction::AcceptGoal(goal_id) => {
                self.dispatch(DesktopIntent::InvokeCommand {
                    id: CommandId::BuiltinPath {
                        root: BuiltinCommand::Goal,
                        path: vec!["goal", "accept"],
                    },
                    args: CommandArguments {
                        tokens: vec![goal_id, value],
                    },
                });
            }
            FormAction::McpAddChoose(_) => {}
            FormAction::McpAddTransport(_) | FormAction::McpAddCommand(_) => {}
            FormAction::RemoveProject(project_id) => {
                if value == "remove" {
                    if self.session_project_filter.as_ref() == Some(&project_id) {
                        self.session_project_filter = None;
                    }
                    self.collapsed_session_projects.remove(&project_id);
                    self.dispatch(DesktopIntent::RemoveProject { project_id });
                }
            }
            FormAction::ConfirmCancelRun => {
                if value == "stop" {
                    self.dispatch(DesktopIntent::CancelRun);
                }
            }
            FormAction::McpAddUrl(name) => {
                self.dispatch(DesktopIntent::InvokeCommand {
                    id: CommandId::Cli(tact_catalog::CliEntry::McpAdd),
                    args: CommandArguments {
                        tokens: vec![name, "--url".into(), value],
                    },
                });
            }
            FormAction::McpPrompt { server, name } => {
                let mut tokens = vec![server, name];
                tokens.extend(value.split_whitespace().map(ToOwned::to_owned));
                self.execute_command(
                    CommandId::BuiltinPath {
                        root: BuiltinCommand::Mcp,
                        path: vec!["mcp", "prompt"],
                    },
                    CommandArguments { tokens },
                    window,
                    cx,
                );
            }
            FormAction::Command(CommandId::Setting(setting)) => {
                let value = setting_value(setting, &value);
                if let FormValue::Setting(id, value) = value {
                    self.dispatch(DesktopIntent::SetSetting { id, value });
                }
                self.sync_prompt_state(cx);
            }
            FormAction::Command(id) => {
                let tokens = value.split_whitespace().map(ToOwned::to_owned).collect();
                self.execute_command(id, CommandArguments { tokens }, window, cx);
            }
        }
    }

    fn on_command_search(
        &mut self,
        event: &CommandSearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            CommandSearchEvent::Selected { item_id, .. } => {
                if let Some(command) = find_command(item_id.as_ref()) {
                    let goal_list = matches!(
                        &command.id,
                        CommandId::BuiltinPath { path, .. }
                            if path.as_slice() == ["goal", "list"]
                    );
                    if goal_list {
                        self.command_scope = None;
                        self.close_overlay();
                        self.restore_focus(window, cx);
                        self.execute_command(
                            command.id.clone(),
                            CommandArguments::default(),
                            window,
                            cx,
                        );
                        return;
                    }
                    if !command.children.is_empty() {
                        self.command_scope = Some(command.id.clone());
                        self.rebuild_command_search(window, cx, "");
                        self.command_search.update(cx, |search, cx| {
                            search.focus(window, cx);
                        });
                        return;
                    }
                    if matches!(
                        command.id,
                        CommandId::Builtin(
                            BuiltinCommand::Model
                                | BuiltinCommand::ModelSubagent
                                | BuiltinCommand::Permission
                        )
                    ) {
                        self.command_scope = None;
                        self.close_overlay();
                        self.restore_focus(window, cx);
                        self.execute_command(
                            command.id.clone(),
                            CommandArguments::default(),
                            window,
                            cx,
                        );
                        return;
                    }
                    if command.args.is_empty() {
                        self.command_scope = None;
                        self.close_overlay();
                        self.restore_focus(window, cx);
                        self.execute_command(
                            command.id.clone(),
                            CommandArguments::default(),
                            window,
                            cx,
                        );
                    } else {
                        self.command_scope = None;
                        self.remember_focus(window, cx);
                        self.open_form(command, window, cx);
                    }
                }
            }
            CommandSearchEvent::Dismissed { .. } => {
                self.command_scope = None;
                self.close_overlay();
                self.restore_focus(window, cx);
                cx.notify();
            }
            CommandSearchEvent::QueryChanged { query, .. } => {
                self.rebuild_command_search(window, cx, query);
            }
        }
    }

    fn rebuild_command_search(&mut self, window: &mut Window, cx: &mut Context<Self>, query: &str) {
        self.command_query = query.to_string();
        let mut entries = if let Some(parent) = self.command_scope.as_ref() {
            let catalog = tact_catalog::builtins();
            let parent_path = catalog
                .get(parent)
                .map(|entry| entry.path.as_slice())
                .unwrap_or_default();
            let mut children = catalog
                .entries()
                .filter(|entry| {
                    entry.parent.as_ref() == Some(parent)
                        || (matches!(entry.id, CommandId::Cli(_))
                            && entry.path.len() > parent_path.len()
                            && entry
                                .path
                                .iter()
                                .zip(parent_path)
                                .all(|(left, right)| left == right))
                })
                .collect::<Vec<_>>();
            if !query.trim().is_empty() {
                let locale = self.snapshot.language;
                let mut ranked = children
                    .drain(..)
                    .filter_map(|entry| {
                        let score = tact_catalog::fuzzy_score(query, entry.label.get(locale))
                            .or_else(|| tact_catalog::fuzzy_score(query, &entry.path.join(" ")))
                            .or_else(|| {
                                tact_catalog::fuzzy_score(query, entry.description.get(locale))
                            })
                            .or_else(|| {
                                tact_catalog::fuzzy_score(query, entry.group.label().get(locale))
                            })?;
                        Some((score, entry))
                    })
                    .collect::<Vec<_>>();
                ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
                children = ranked.into_iter().map(|(_, entry)| entry).collect();
            }
            let mut seen = std::collections::HashSet::new();
            children.retain(|entry| seen.insert(tact_desktop_core::canonical_key(&entry.action)));
            children
        } else if query.trim().is_empty() && !self.recent_commands.is_empty() {
            self.recent_commands
                .iter()
                .filter_map(|id| tact_catalog::builtins().get(id))
                .collect::<Vec<_>>()
        } else if query.trim().is_empty() {
            tact_catalog::builtins()
                .entries()
                .filter(|entry| entry.parent.is_none())
                .take(8)
                .collect::<Vec<_>>()
        } else {
            tact_desktop_core::search_commands(
                tact_catalog::builtins(),
                query,
                self.snapshot.language,
            )
        };
        if query.trim().is_empty() && self.command_scope.is_none() {
            entries.truncate(8);
        }
        let availability = self.command_availability();
        let locale = self.snapshot.language;
        let items = entries
            .into_iter()
            .map(|entry| {
                let mut keywords = entry.aliases.clone();
                keywords.push(entry.path.join(" "));
                // The prototype's rows read "GROUP · description" and carry
                // the invocation on the right, so the palette teaches the
                // command instead of only naming it.
                let group = entry.group.label().get(locale);
                let subtitle = format!(
                    "{} · {}",
                    group.to_uppercase(),
                    entry.description.get(locale)
                );
                let hint = match &entry.id {
                    CommandId::Builtin(_) | CommandId::BuiltinPath { .. } => {
                        format!("/{}", entry.path.join(" "))
                    }
                    CommandId::Setting(_) => "setting".to_string(),
                    CommandId::Cli(_) => format!("tact {}", entry.path.join(" ")),
                    CommandId::Composer(_) => "composer".to_string(),
                    CommandId::DynamicSkill(_) => "skill".to_string(),
                };
                let icon = match entry.group {
                    tact_catalog::CommandGroup::Session => IconName::PanelLeft,
                    tact_catalog::CommandGroup::Run => IconName::Play,
                    tact_catalog::CommandGroup::Appearance => IconName::Palette,
                    tact_catalog::CommandGroup::Account => IconName::CircleUser,
                    tact_catalog::CommandGroup::Goals => IconName::CircleCheck,
                    tact_catalog::CommandGroup::Extensions => IconName::LayoutDashboard,
                    tact_catalog::CommandGroup::Readers => IconName::BookOpen,
                    tact_catalog::CommandGroup::Composer => IconName::PanelRight,
                    tact_catalog::CommandGroup::Cli => IconName::Settings,
                };
                CommandSearchItem::new(command_key(&entry.id), entry.label.get(locale))
                    .icon(icon.path())
                    .subtitle(subtitle)
                    .shortcut(hint)
                    .keywords(keywords)
                    .disabled(!is_enabled(entry, availability))
            })
            .collect::<Vec<_>>();
        self.command_hit_count = items.len();
        let locale = self.snapshot.language;
        self.command_search.update(cx, |search, cx| {
            search.set_labels(command_search_labels(locale), cx);
            search.set_items(items, window, cx);
            search.set_query(query, window, cx);
        });
    }

    fn command_availability(&self) -> CommandAvailability {
        CommandAvailability {
            has_project: self.snapshot.project.id.is_some(),
            has_session: self.snapshot.session.id.is_some(),
            has_account: self.snapshot.account_supported,
            has_installed_skill: !self.snapshot.extensions.snapshot.skills.is_empty(),
        }
    }

    fn open_command_center(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Some(Overlay::CommandCenter)) {
            self.command_scope = None;
        }
        self.rebuild_command_search(window, cx, "");
        if !matches!(self.overlay, Some(Overlay::CommandCenter)) {
            self.open_overlay(Overlay::CommandCenter);
        }
        self.command_search
            .update(cx, |search, cx| search.focus(window, cx));
        self.overlay_focus_pending = false;
        cx.notify();
    }

    fn remember_command(&mut self, id: CommandId) {
        self.recent_commands.retain(|candidate| candidate != &id);
        self.recent_commands.insert(0, id);
        self.recent_commands.truncate(8);
    }

    fn attachments(&self) -> Vec<PathBuf> {
        self.attachments.clone()
    }

    fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        self.platform_prompts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(self.text(TextKey::AttachFiles).into()),
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = weak.update(cx, |this, cx| {
                    this.attachments.extend(paths);
                    this.sync_prompt_attachments(cx);
                });
            }
        })
        .detach();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        self.pick_attachments(cx);
    }

    fn sync_prompt_attachments(&mut self, cx: &mut Context<Self>) {
        let attachments = self
            .attachments
            .iter()
            .map(|path| {
                let mut item = AttachmentItem::new(
                    path.display().to_string(),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(self.text(TextKey::Attachment)),
                );
                if is_image_path(path) {
                    item = item.image(path.clone());
                }
                item.size_bytes(std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
            })
            .collect::<Vec<_>>();
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_attachments(attachments, cx);
        });
    }

    fn dismiss_topmost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A pending gate ignores Esc unless it denies the request: the inline
        // card is the review surface, and leaving it unanswered would block
        // the run behind a prompt the user has dismissed.
        if self.overlay.is_none()
            && let Some(gate) = self.snapshot.pending_gates.first()
            && gate.status == GateState::Pending
        {
            self.dispatch(DesktopIntent::RespondGate {
                session_epoch: gate.session_epoch,
                request_id: gate.request_id.0,
                response: match gate.kind {
                    GateKind::Select => GateResponse::Select(None),
                    GateKind::MultiSelect => GateResponse::MultiSelect(None),
                },
            });
            self.submitted_gates.insert(gate.request_id.0);
            self.restore_focus(window, cx);
            cx.notify();
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay();
            self.restore_focus(window, cx);
            cx.notify();
            return;
        }
        // Esc interrupts a live run before it hides navigation chrome.
        if matches!(
            self.snapshot.session.phase,
            SessionPhase::Running | SessionPhase::Starting | SessionPhase::Opening
        ) {
            self.request_cancel_run(window, cx);
            return;
        }
        let layout = crate::ui::layout::layout(
            f32::from(window.viewport_size().width),
            self.snapshot.text_scale,
            self.left_visible,
            self.right_visible,
        );
        if !layout.three_column {
            if self.left_visible {
                self.left_visible = false;
                self.restore_focus(window, cx);
                cx.notify();
                return;
            }
            if self.right_visible {
                self.right_visible = false;
                self.restore_focus(window, cx);
                cx.notify();
                return;
            }
        }
        self.request_cancel_run(window, cx);
    }

    fn destructive_tool_active(&self) -> bool {
        self.snapshot.transcript.iter().any(|row| match &row.body {
            RowBody::ToolGroup(group) => group.items.iter().any(|item| {
                matches!(
                    item,
                    ToolGroupItem::Tool(tool)
                        if tool.status == RowStatus::Running
                            && (tool.destructive
                                || matches!(
                                    tool.body,
                                    tact_desktop_core::ToolBody::Diff { .. }
                                ))
                )
            }),
            _ => false,
        })
    }

    fn request_cancel_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.destructive_tool_active() {
            self.dispatch(DesktopIntent::CancelRun);
            return;
        }
        self.remember_focus(window, cx);
        self.set_form(
            FormState {
                title: self.text(TextKey::StopRun).into(),
                action: FormAction::ConfirmCancelRun,
                field: None,
                required: false,
                choice: Some(vec![
                    (self.text(TextKey::StopRun).into(), "stop".into()),
                    (self.text(TextKey::KeepRunning).into(), "keep".into()),
                ]),
            },
            window,
            cx,
        );
    }

    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = self.focus_restore.restore() {
            focus.focus(window, cx);
        }
    }

    fn open_project_picker(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.text(TextKey::OpenProjectFolder).into()),
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(root) = paths.pop()
            {
                let _ = weak.update(cx, |this, _| {
                    this.dispatch(DesktopIntent::OpenProject { root });
                });
            }
        })
        .detach();
    }

    fn open_remove_project(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .snapshot
            .projects
            .iter()
            .find(|project| project.id.as_ref() == Some(&project_id))
            .map(project_identity_label)
            .unwrap_or_else(|| project_id.0.clone());
        self.remember_focus(window, cx);
        self.set_form(
            FormState {
                title: self.text(TextKey::RemoveProject).into(),
                action: FormAction::RemoveProject(project_id),
                field: Some(format!("{name}\n{}", self.text(TextKey::RemoveProjectHint))),
                required: false,
                choice: Some(vec![
                    (self.text(TextKey::Remove).into(), "remove".into()),
                    (self.text(TextKey::Cancel).into(), "cancel".into()),
                ]),
            },
            window,
            cx,
        );
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = project_identity_label(&self.snapshot.project);
        let session_label = self
            .snapshot
            .session
            .id
            .as_ref()
            .map(|id| {
                let raw = id.0.to_string();
                let short = raw.rsplit('-').next().unwrap_or(&raw);
                format!(
                    "{} {}",
                    self.text(TextKey::SessionShort),
                    &short[..short.len().min(4)]
                )
            })
            .unwrap_or_else(|| self.text(TextKey::NoSession).to_string());
        let appearance_label = match self.snapshot.appearance {
            Appearance::Light => self.text(TextKey::Light),
            Appearance::Dark => self.text(TextKey::Dark),
        };
        let appearance_icon = match self.snapshot.appearance {
            Appearance::Light => IconName::Moon,
            Appearance::Dark => IconName::Sun,
        };
        let geometry = crate::ui::shell::geometry(self.snapshot.appearance);
        TitleBar::new()
            .h(px(f32::from(geometry.titlebar_height)))
            .child(
                h_flex()
                    .id("desktop-title-bar")
                    .debug_selector(|| "desktop-title-bar".into())
                    .role(Role::Toolbar)
                    .aria_label(self.text(TextKey::TitleBar))
                    .size_full()
                    .gap(px(10.))
                    .items_center()
                    .pl(px(0.))
                    .pr(px(18.))
                    .font_family(theme::native_font_family())
                    .border_b(px(theme::border_width()))
                    .border_color(cx.theme().border)
                    .bg(cx.theme().title_bar)
                    .child(
                        h_flex()
                            .id("toggle-sidebar")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::ToggleSidebar))
                            .aria_selected(self.left_visible)
                            .cursor_pointer()
                            .items_center()
                            .justify_center()
                            .size(px(26.))
                            .rounded_md()
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(IconName::PanelLeft)
                                    .size(px(16.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.left_visible = !this.left_visible;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(27.))
                                    .rounded(px(7.))
                                    .bg(cx.theme().primary)
                                    .text_color(cx.theme().primary_foreground)
                                    .text_size(px(18.))
                                    .font_bold()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("T"),
                            )
                            .child(
                                v_flex()
                                    .gap_0()
                                    .child(
                                        div()
                                            .text_size(px(17.))
                                            .font_semibold()
                                            .line_height(px(17.))
                                            .child("tact"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .font_family("Lilex")
                                            .text_color(cx.theme().muted_foreground)
                                            .line_height(px(12.))
                                            .child(
                                                self.text(TextKey::DesktopPreview).to_uppercase(),
                                            ),
                                    ),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .id("workspace-switcher")
                            .debug_selector(|| "workspace-switcher".into())
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::OpenProject))
                            .cursor_pointer()
                            .gap(px(9.))
                            .items_center()
                            .rounded(px(8.))
                            .bg(cx.theme().muted)
                            .px(px(10.))
                            .py(px(7.))
                            .hover(|style| style.bg(cx.theme().border))
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(13.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .max_w(px(220.))
                                    .truncate()
                                    .text_xs()
                                    .font_semibold()
                                    .child(project.to_string()),
                            )
                            .when_some(self.snapshot.project.branch.clone(), |switcher, branch| {
                                switcher
                                    .child(div().h(px(12.)).w(px(1.)).bg(cx.theme().border))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child("⑂"),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(150.))
                                            .truncate()
                                            .text_xs()
                                            .font_family("Lilex")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(branch.to_string()),
                                    )
                            })
                            .child(div().h(px(12.)).w(px(1.)).bg(cx.theme().border))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(session_label),
                            )
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .size(px(12.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.open_project_picker(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .id("appearance-toggle")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::SwitchAppearance))
                            .cursor_pointer()
                            .gap_1()
                            .items_center()
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(appearance_icon)
                                    .size(px(14.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(appearance_label),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                this.dispatch(DesktopIntent::InvokeCommand {
                                    id: CommandId::Builtin(BuiltinCommand::Theme),
                                    args: CommandArguments::default(),
                                });
                            })),
                    )
                    .child(
                        h_flex()
                            .id("command-center-trigger")
                            .debug_selector(|| "command-center-trigger".into())
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::OpenCommandCenter))
                            .cursor_pointer()
                            .gap_1()
                            .items_center()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().popover)
                            .px_2()
                            .py_1()
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(IconName::Settings2)
                                    .size(px(13.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(div().text_xs().child(self.text(TextKey::Commands)))
                            .child(
                                div()
                                    .rounded_md()
                                    .bg(cx.theme().muted)
                                    .px_1()
                                    .text_xs()
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(keymap::command_center_hint()),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.open_command_center(window, cx);
                            })),
                    )
                    .child(
                        // UI-parity placeholder until account identity is wired.
                        div()
                            .id("account-avatar")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .aria_label(self.text(TextKey::Account))
                            .cursor_pointer()
                            .flex_none()
                            .size(px(31.))
                            .rounded(px(8.))
                            .bg(cx.theme().foreground)
                            .text_color(cx.theme().background)
                            .text_size(px(10.))
                            .font_semibold()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(local_user_initials()),
                    )
                    .child(
                        div()
                            .id("toggle-inspector")
                            .debug_selector(|| "toggle-inspector".into())
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::ToggleInspector))
                            .aria_selected(self.right_visible)
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(26.))
                            .rounded_md()
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(IconName::PanelRight)
                                    .size(px(16.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                if !this.right_visible {
                                    this.remember_focus(window, cx);
                                }
                                this.right_visible = !this.right_visible;
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_divider(&self, side: DividerSide, cx: &mut Context<Self>) -> AnyElement {
        let id = match side {
            DividerSide::Rail => "rail-divider",
            DividerSide::Inspector => "inspector-divider",
        };
        let label = match (self.snapshot.language, side) {
            (tact_catalog::CatalogLocale::Zh, DividerSide::Rail) => "拖动调整会话栏宽度",
            (tact_catalog::CatalogLocale::Zh, DividerSide::Inspector) => "拖动调整检查器宽度",
            (_, DividerSide::Rail) => "Resize session rail",
            (_, DividerSide::Inspector) => "Resize inspector",
        };
        let active = self.divider_drag.is_some_and(|drag| drag.side == side);
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Button)
            .aria_label(label)
            .flex_none()
            .w(px(7.))
            .h_full()
            .relative()
            .cursor_col_resize()
            .hover(|style| style.bg(cx.theme().muted.opacity(0.55)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.divider_drag = Some(DividerDrag {
                        side,
                        pointer_x: f32::from(event.position.x),
                        width: match side {
                            DividerSide::Rail => this.rail_width,
                            DividerSide::Inspector => this.inspector_width,
                        },
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(3.))
                    .top_0()
                    .h_full()
                    .w(px(1.))
                    .bg(if active {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    }),
            )
            .into_any_element()
    }

    fn render_session_rail(
        &self,
        width: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The session list is the rail's main content: it stretches to fill
        // whatever the pinned blocks below leave, so every pixel the removed
        // DURABLE block freed goes to the sessions. Their height is
        // `flex_none` chrome; the list's `min_h` keeps a short window usable.
        self.sync_sidebar_nav(window, cx);

        v_flex()
            .id("session-rail")
            .debug_selector(|| "session-rail".into())
            .role(Role::Navigation)
            .aria_label(self.text(TextKey::ProjectsAndSessions))
            .w(width)
            .h_full()
            .pt(px(14.))
            .px(px(12.))
            .pb(px(12.))
            .gap_1()
            .font_family(theme::native_font_family())
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .id("rail-search")
                    .debug_selector(|| "rail-search".into())
                    .w_full()
                    .h(px(33.))
                    .mt(px(8.))
                    .items_center()
                    .child(
                        surface::quiet_search_input(&self.rail_search, window, cx)
                            .w_full()
                            .prefix(
                                Icon::new(IconName::Search)
                                    .size(px(13.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .suffix(
                                div()
                                    .flex_none()
                                    .rounded(px(5.))
                                    .bg(cx.theme().popover)
                                    .px(px(5.))
                                    .py(px(1.))
                                    .text_size(px(10.))
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child("/"),
                            )
                            .aria_label(self.text(TextKey::SearchSessions)),
                    ),
            )
            .child(
                h_flex()
                    .id("rail-sessions-header")
                    .w_full()
                    .pl(px(14.))
                    .mt(px(10.))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::Sessions).to_uppercase()),
                    )
                    .child(
                        div()
                            .id("manage-sessions")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::ManageSessions))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .hover(|style| style.text_color(cx.theme().foreground))
                            .child(self.text(TextKey::Manage))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.session_project_filter = this.snapshot.project.id.clone();
                                this.session_search.update(cx, |input, cx| {
                                    input.focus(window, cx);
                                });
                                this.open_overlay(Overlay::SessionManager);
                                this.overlay_focus_pending = false;
                            })),
                    ),
            )
            .when_some(self.snapshot.sessions.error.clone(), |rail, failure| {
                rail.child(
                    v_flex()
                        .id("session-list-error")
                        .role(Role::Alert)
                        .aria_label(failure.message.to_string())
                        .gap_1()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().danger)
                        .p_2()
                        .text_xs()
                        .child(failure.message.to_string())
                        .child(
                            div()
                                .id("retry-session-list")
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(self.text(TextKey::Retry))
                                .cursor_pointer()
                                .text_color(cx.theme().link)
                                .child(self.text(TextKey::Retry))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                    this.dispatch(DesktopIntent::RefreshSessions);
                                })),
                        ),
                )
            })
            .when(self.snapshot.sessions.loading, |rail| {
                rail.child(LoadingState::new().label(match self.snapshot.language {
                    tact_catalog::CatalogLocale::En => "Loading sessions",
                    tact_catalog::CatalogLocale::Zh => "正在加载会话",
                }))
            })
            .child(
                div()
                    .id("session-list")
                    .debug_selector(|| "session-list".into())
                    .min_h_0()
                    // The list absorbs every pixel the rail has left once the
                    // pinned blocks are placed: quota and Settings sit at the
                    // bottom, so spare height is the sessions'.
                    .flex_1()
                    .min_h(px(36. * self.snapshot.text_scale))
                    .mt(px(6.))
                    .child(self.sidebar_nav.clone()),
            )
            .child(
                h_flex()
                    .id("view-all-sessions")
                    .debug_selector(|| "view-all-sessions".into())
                    .w_full()
                    .h(px(28.))
                    .mt(px(4.))
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().child(self.text(TextKey::ViewAllSessions)))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .child(self.snapshot.sessions.items.len().to_string()),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        this.session_project_filter = this.snapshot.project.id.clone();
                        this.session_search.update(cx, |input, cx| {
                            input.focus(window, cx);
                        });
                        this.open_overlay(Overlay::SessionManager);
                        this.overlay_focus_pending = false;
                        cx.notify();
                    })),
            )
            .when_some(
                self.account_summary(),
                |rail, (label, detail, ratio, resets)| {
                    rail.child(
                        v_flex()
                            .gap_1()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(cx.theme().popover)
                            .p_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(11.))
                                            .font_semibold()
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_family("Lilex")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(detail),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(4.))
                                    .w_full()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .child(
                                        div()
                                            .h_full()
                                            .w(gpui::relative(ratio.clamp(0.0, 1.0)))
                                            .rounded_full()
                                            .bg(cx.theme().success),
                                    ),
                            )
                            .when_some(resets, |card, resets| {
                                card.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(match self.snapshot.language {
                                            tact_catalog::CatalogLocale::En => {
                                                format!("Quota resets {resets}")
                                            }
                                            tact_catalog::CatalogLocale::Zh => {
                                                format!("配额将在 {resets} 重置")
                                            }
                                        }),
                                )
                            }),
                    )
                },
            )
            .child(
                h_flex()
                    .id("open-settings")
                    .debug_selector(|| "open-settings".into())
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(self.text(TextKey::Settings))
                    .cursor_pointer()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        Icon::new(IconName::Settings)
                            .size(px(14.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().flex_1().text_xs().child(self.text(TextKey::Settings)))
                    .child(
                        div()
                            .text_xs()
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(keymap::settings_hint()),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        this.open_overlay(Overlay::Settings);
                    })),
            )
    }

    fn account_summary(&self) -> Option<(String, String, f32, Option<String>)> {
        match self.snapshot.account.as_ref()? {
            tact_desktop_core::AccountView::UsageQuota {
                plan,
                used,
                limit,
                resets_at,
            } => {
                let ratio = match (used, limit) {
                    (Some(used), Some(limit)) if *limit > 0 => *used as f32 / *limit as f32,
                    _ => 0.0,
                };
                let label = plan
                    .as_deref()
                    .map(str::to_string)
                    .unwrap_or_else(|| self.text(TextKey::Quota).to_string());
                let detail = format!("{:.0}%", ratio.clamp(0.0, 1.0) * 100.0);
                Some((
                    label,
                    detail,
                    ratio,
                    resets_at.as_deref().map(str::to_string),
                ))
            }
            tact_desktop_core::AccountView::Balance { total, .. } => Some((
                self.text(TextKey::Balance).to_string(),
                total.to_string(),
                1.0,
                None,
            )),
            tact_desktop_core::AccountView::Error(_) => None,
        }
    }

    fn transcript_session_id(&self) -> SessionId {
        self.snapshot
            .session
            .id
            .clone()
            .unwrap_or_else(|| SessionId(format!("epoch-{}", self.snapshot.session.epoch.0)))
    }

    fn touch_transcript_session_cache(&mut self, session_id: &SessionId) {
        self.transcript_session_cache
            .retain(|cached| cached != session_id);
        self.transcript_session_cache.push_front(session_id.clone());
        while self.transcript_session_cache.len() > TRANSCRIPT_SESSION_CACHE {
            let Some(evicted) = self.transcript_session_cache.pop_back() else {
                break;
            };
            self.assistant_selections
                .retain(|(_, cached_session, _), _| cached_session != &evicted);
            self.transcript_rows
                .retain(|(cached_session, _), _| cached_session != &evicted);
            self.expanded_thinking
                .retain(|(cached_session, _)| cached_session != &evicted);
        }
    }

    fn sync_assistant_selections(
        &mut self,
        transcript: &tact_desktop_core::TranscriptView,
        scope: &str,
        session_id: &SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashMap<(String, SessionId, RowId), Entity<SelectionActions>> {
        fn assistant_text_style(appearance: Appearance) -> TextViewStyle {
            TextViewStyle {
                paragraph_gap: gpui::rems(0.56),
                heading_font_size: Some(std::sync::Arc::new(|level, _base| match level {
                    1 => px(21.),
                    2 => px(18.),
                    3 => px(16.),
                    _ => px(13.),
                })),
                is_dark: appearance == Appearance::Dark,
                ..Default::default()
            }
        }

        fn configure_selection(
            selection: &Entity<SelectionActions>,
            language: tact_catalog::CatalogLocale,
            appearance: Appearance,
            cx: &mut Context<DesktopRoot>,
        ) {
            selection.update(cx, |selection, cx| {
                selection.set_toolbar_label(i18n::text(language, TextKey::SelectionActions), cx);
                selection.set_actions(
                    [
                        SelectionAction::new("ask", i18n::text(language, TextKey::Ask)),
                        SelectionAction::new("explain", i18n::text(language, TextKey::Explain)),
                        SelectionAction::new("rewrite", i18n::text(language, TextKey::Rewrite)),
                    ],
                    cx,
                );
                selection.set_text_style(assistant_text_style(appearance), cx);
            });
        }

        let language = self.snapshot.language;
        let appearance = self.snapshot.appearance;
        let mut live = HashSet::new();
        let mut selections = HashMap::new();
        for row in transcript.iter() {
            let RowBody::Assistant(assistant) = &row.body else {
                continue;
            };
            live.insert(row.id);
            let key = (scope.to_string(), session_id.clone(), row.id);
            let cached = self.assistant_selections.get(&key).map(|entry| {
                (
                    entry.row.clone(),
                    entry.selection.clone(),
                    entry.language,
                    entry.appearance,
                )
            });
            if let Some((cached_row, selection, cached_language, cached_appearance)) = cached {
                if Arc::ptr_eq(&cached_row, row) {
                    if cached_language != language || cached_appearance != appearance {
                        configure_selection(&selection, language, appearance, cx);
                        if let Some(entry) = self.assistant_selections.get_mut(&key) {
                            entry.language = language;
                            entry.appearance = appearance;
                        }
                    }
                    selections.insert(key, selection);
                    continue;
                }
                let display_markdown = assistant_display_markdown(
                    &assistant.markdown,
                    self.snapshot.project.root.as_deref(),
                );
                selection.update(cx, |selection, cx| {
                    selection.set_markdown(display_markdown, cx);
                });
                if cached_language != language || cached_appearance != appearance {
                    configure_selection(&selection, language, appearance, cx);
                }
                self.assistant_selections.insert(
                    key.clone(),
                    AssistantSelection {
                        row: row.clone(),
                        selection: selection.clone(),
                        language,
                        appearance,
                    },
                );
                selections.insert(key, selection);
                continue;
            }
            let display_markdown = assistant_display_markdown(
                &assistant.markdown,
                self.snapshot.project.root.as_deref(),
            );
            let selection = cx.new(|cx| {
                SelectionActions::new(
                    format!("assistant-selection-{}", row.ordinal),
                    display_markdown.clone(),
                    window,
                    cx,
                )
                // The design draws an assistant answer as prose on the
                // transcript canvas; only the floating selection toolbar is
                // a surface.
                .frameless()
                .natural_height()
            });
            configure_selection(&selection, language, appearance, cx);
            let subscription = cx.subscribe_in(
                &selection,
                window,
                |this, _, event: &SelectionActionsEvent, window, cx| {
                    let SelectionActionsEvent::Invoked {
                        action_id,
                        selected_text,
                        ..
                    } = event;
                    let prefix = match action_id.as_ref() {
                        "explain" => this.text(TextKey::Explain),
                        "rewrite" => this.text(TextKey::Rewrite),
                        _ => this.text(TextKey::Ask),
                    };
                    this.prompt.update(cx, |prompt, cx| {
                        prompt.set_draft(format!("{prefix}: {selected_text}"), window, cx);
                        prompt.focus(window, cx);
                    });
                },
            );
            self._subscriptions.push(subscription);
            self.assistant_selections.insert(
                key.clone(),
                AssistantSelection {
                    row: row.clone(),
                    selection: selection.clone(),
                    language,
                    appearance,
                },
            );
            selections.insert(key, selection);
        }
        self.assistant_selections
            .retain(|(key_scope, key_session, row_id), _| {
                key_session != session_id || key_scope != scope || live.contains(row_id)
            });
        selections
    }

    /// The run room's page tabs: the conversation and the trajectory ledger
    /// are two views of the same rows, not two sessions.
    fn render_run_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let labels = [
            (RunTab::Chat, self.text(TextKey::ChatTab)),
            (RunTab::Trajectory, self.text(TextKey::TrajectoryTab)),
        ];
        h_flex()
            .id("run-tabs")
            .debug_selector(|| "run-tabs".into())
            .role(Role::TabList)
            .aria_label(self.text(TextKey::TrajectoryTab))
            .w_full()
            .flex_none()
            .gap(px(2.))
            .border_b_1()
            .border_color(cx.theme().border)
            .children(labels.into_iter().enumerate().map(|(index, (tab, label))| {
                let selected = self.run_tab == tab;
                div()
                    .id(SharedString::from(format!("run-tab-{index}")))
                    .debug_selector(move || format!("run-tab-{index}"))
                    .role(Role::Tab)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_selected(selected)
                    .aria_label(label)
                    .cursor_pointer()
                    .px(px(12.))
                    .pt(px(9.))
                    .pb(px(8.))
                    .text_size(px(12.5))
                    .font_semibold()
                    .when(selected, |tab| {
                        tab.text_color(cx.theme().foreground)
                            .border_b_2()
                            .border_color(cx.theme().primary)
                    })
                    .when(!selected, |tab| {
                        tab.text_color(cx.theme().muted_foreground)
                            .hover(|style| style.text_color(cx.theme().foreground))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.select_run_tab(tab, cx);
                    }))
            }))
            .when(self.run_tab == RunTab::Chat, |strip| {
                strip.child(div().flex_1()).child(
                    div()
                        .id("run-tab-answers-only")
                        .debug_selector(|| "run-tab-answers-only".into())
                        .role(Role::Button)
                        .aria_label(self.text(TextKey::AnswersOnly))
                        .aria_selected(self.answers_only)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                        .cursor_pointer()
                        .mr(px(8.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .text_size(px(11.))
                        .when(self.answers_only, |button| {
                            button
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().foreground)
                        })
                        .when(!self.answers_only, |button| {
                            button
                                .text_color(cx.theme().muted_foreground)
                                .hover(|style| style.text_color(cx.theme().foreground))
                        })
                        .child(self.text(TextKey::AnswersOnly))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.answers_only = !this.answers_only;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    /// Header figures for the ledger: how many turns, records and calls the
    /// run produced, and the duration it can actually back with evidence.
    fn render_trajectory_summary(
        &self,
        view: &TrajectoryView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let line = trajectory_page::SummaryLine::new(view.summary(), self.text(TextKey::Measured));
        let metric = |label: &'static str, value: String, cx: &mut Context<Self>| {
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_family("Lilex")
                        .text_color(cx.theme().foreground)
                        .child(value),
                )
        };
        h_flex()
            .id("trajectory-summary")
            .debug_selector(|| "trajectory-summary".into())
            .w_full()
            .flex_none()
            .gap(px(18.))
            .items_center()
            .child(metric(self.text(TextKey::Duration), line.measured, cx))
            .child(metric(self.text(TextKey::Turns), line.turns, cx))
            .child(metric(self.text(TextKey::Calls), line.calls, cx))
            .child(metric(self.text(TextKey::RecordPrefix), line.records, cx))
            .child(div().flex_1())
            .child(
                surface::quiet_search_input(&self.trajectory_search, window, cx)
                    .prefix(
                        Icon::new(IconName::Search)
                            .size(px(12.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .xsmall()
                    .w(px(180.))
                    .aria_label(self.text(TextKey::SearchTrajectory)),
            )
            .into_any_element()
    }

    /// The network-plot-style overview: every record as one block in the lane
    /// its kind belongs to, with a rule at each turn boundary.
    ///
    /// The plot is one `canvas`: paints are cheap for a ledger of hundreds of
    /// records. Paint and click both use `TimelineLayout`, so the visible block
    /// and its hit target cannot drift apart.
    fn render_trajectory_overview(
        &mut self,
        view: &Arc<TrajectoryView>,
        matches: Option<&[usize]>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let match_set: Option<HashSet<usize>> =
            matches.map(|matches| matches.iter().map(|index| index - 1).collect());
        let has_filter = match_set.is_some();
        let selected = self.trajectory_selected.and_then(|id| view.position_of(id));
        let palette = TrajectoryPalette::new(cx);
        let layout_view = Arc::clone(view);
        let track = self.trajectory_track.clone();
        let plot = canvas(
            move |bounds, _, _| {
                track.set((f32::from(bounds.origin.x), f32::from(bounds.size.width)));
                bounds
            },
            move |bounds, _, window, _| {
                let track_left = bounds.origin.x + px(trajectory_page::LABELS_WIDTH);
                let track_width = f32::from(bounds.size.width) - trajectory_page::LABELS_WIDTH;
                if track_width <= 0. || layout_view.is_empty() {
                    return;
                }
                let layout = trajectory_page::TimelineLayout::new(&layout_view, track_width);
                // Turn rules sit behind the blocks so a boundary reads as a
                // divider, not as another record.
                for boundary in &layout.turn_marks {
                    let left = track_left + px(layout.offset_of(*boundary));
                    window.paint_quad(paint_rect(
                        left,
                        bounds.origin.y,
                        px(1.),
                        bounds.size.height,
                        palette.rule,
                        px(0.),
                        None,
                    ));
                }
                for span in &layout.boxes {
                    let Some(source) = layout_view.spans().get(span.index) else {
                        continue;
                    };
                    let matched = match_set
                        .as_ref()
                        .is_some_and(|matches| matches.contains(&source.index));
                    let color = if source.is_error {
                        palette.error
                    } else {
                        palette.lanes[source.lane.row()]
                    };
                    let color = if has_filter && !matched {
                        palette.dim(color)
                    } else {
                        color
                    };
                    window.paint_quad(paint_rect(
                        track_left + px(span.left),
                        bounds.origin.y + px(span.top),
                        px(span.width),
                        px(trajectory_page::BLOCK_HEIGHT),
                        color,
                        px(1.),
                        None,
                    ));
                }
                if let Some(span) = selected.and_then(|index| layout.boxes.get(index)) {
                    window.paint_quad(paint_rect(
                        track_left + px(span.left),
                        bounds.origin.y,
                        px(span.width),
                        bounds.size.height,
                        palette.selection,
                        px(1.),
                        Some(palette.accent),
                    ));
                }
            },
        )
        .absolute()
        .inset_0();
        let labels: Vec<&'static str> =
            [TextKey::InputLane, TextKey::ModelLane, TextKey::ToolsLane]
                .into_iter()
                .map(|key| self.text(key))
                .collect();
        let lanes = v_flex()
            .w(px(trajectory_page::LABELS_WIDTH))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .children(labels.into_iter().enumerate().map(|(index, label)| {
                div()
                    .h(px(trajectory_page::LANE_HEIGHT))
                    .when(index == 0, |row| row.mt(px(trajectory_page::LANE_TOP)))
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr(px(4.))
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child(label)
            }));
        let track_width = self.trajectory_track.clone();
        h_flex()
            .id("trajectory-overview")
            .debug_selector(|| "trajectory-overview".into())
            .role(Role::Group)
            .aria_label(self.text(TextKey::Overview))
            .w_full()
            .flex_none()
            .h(px(trajectory_page::PLOT_HEIGHT))
            .items_start()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(6.))
            .overflow_hidden()
            .bg(cx.theme().group_box)
            .child(lanes)
            .child(
                div()
                    .id("trajectory-overview-plot")
                    .debug_selector(|| "trajectory-overview-plot".into())
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .cursor_pointer()
                    .child(plot)
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        let (origin, width) = track_width.get();
                        if width <= 0. {
                            return;
                        }
                        let local =
                            f32::from(event.position().x) - origin - trajectory_page::LABELS_WIDTH;
                        let layout_view = this.trajectory_view();
                        let layout = trajectory_page::TimelineLayout::new(&layout_view, width);
                        if let Some(index) = layout.hit(local)
                            && let Some(id) = layout_view.record(index).map(|record| record.id)
                        {
                            this.select_trajectory_record(id, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    /// The Trajectory page: overview, ledger, and the selected record's detail.
    ///
    /// Everything here is derived from the same transcript the Chat page
    /// renders, so the two pages can never disagree about what the run did.
    fn render_trajectory_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = self.trajectory_view();
        if view.is_empty() {
            return v_flex()
                .id("empty-trajectory")
                .size_full()
                .items_center()
                .justify_center()
                .p_8()
                .child(
                    v_flex()
                        .w(px(520.))
                        .max_w_full()
                        .gap_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .p_6()
                        .child(
                            div()
                                .text_xl()
                                .font_semibold()
                                .child(self.text(TextKey::NoTrajectory)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.text(TextKey::NoTrajectoryBody)),
                        ),
                )
                .into_any_element();
        }
        let visible = self.trajectory_visible_indexes(&view);
        self.sync_trajectory_scroller(cx);
        let rows = self.trajectory_list_rows(&view);
        self.trajectory_selected_index = rows
            .iter()
            .position(|row| Some(row.id) == self.trajectory_selected);
        let visible_count = rows.len();
        let list_state = self.trajectory_scroller.read(cx).list_state.clone();
        let row_lookup = Rc::new(rows);
        let weak = cx.weak_entity();
        let language = self.snapshot.language;
        let selected = self.trajectory_selected_index;
        let palette = LedgerPalette::new(cx);
        let list = list(list_state.clone(), move |index, _, _| {
            let Some(row) = row_lookup.get(index) else {
                return div().into_any_element();
            };
            ledger_row_element(
                row,
                selected == Some(index),
                weak.clone(),
                palette,
                language,
            )
        })
        .size_full()
        .min_h_0();
        let summary = self.render_trajectory_summary(&view, window, cx);
        let overview = self.render_trajectory_overview(&view, visible.as_deref(), cx);
        let trajectory_detail = self.render_trajectory_detail(&view, cx);
        let subtitle = if self.trajectory_query.trim().is_empty() {
            None
        } else if visible_count == 0 {
            Some(self.text(TextKey::NoMatchingRecords).to_string())
        } else {
            Some(format!("{visible_count} / {}", view.len()))
        };
        v_flex()
            .id("trajectory-page")
            .debug_selector(|| "trajectory-page".into())
            .size_full()
            .min_h_0()
            .gap(px(10.))
            .px(px(20.))
            .pt(px(14.))
            .pb(px(10.))
            .child(h_flex().w_full().items_center().gap(px(12.)).child(summary))
            .child(overview)
            .when_some(subtitle, |page, subtitle| {
                page.child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle),
                )
            })
            .child(
                h_flex()
                    .id("trajectory-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_stretch()
                    .gap(px(12.))
                    .child(
                        v_flex()
                            .id("trajectory-ledger")
                            .debug_selector(|| "trajectory-ledger".into())
                            .role(Role::Log)
                            .aria_label(self.text(TextKey::TrajectoryTab))
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .h_full()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().background)
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(list)
                                    .vertical_scrollbar(&list_state),
                            ),
                    )
                    .when_some(trajectory_detail, |body, detail| body.child(detail)),
            )
            .child(self.render_trajectory_stats(&view, cx))
            .into_any_element()
    }

    /// The page footer: session totals the reference shows under the ledger.
    ///
    /// Each figure is dropped when its producer reported nothing to base it
    /// on, so the bar never shows a zero it cannot back.
    fn render_trajectory_stats(&self, view: &TrajectoryView, cx: &mut Context<Self>) -> AnyElement {
        let line = trajectory_page::StatsLine::new(
            view.summary(),
            trajectory_page::StatsLabels {
                turn: self.text(TextKey::TurnLabel),
                turns: self.text(TextKey::TurnsUnit),
                step: match self.snapshot.language {
                    tact_catalog::CatalogLocale::En => "step",
                    tact_catalog::CatalogLocale::Zh => self.text(TextKey::Steps),
                },
                steps: self.text(TextKey::Steps),
                tokens: self.text(TextKey::Tokens),
                cache_hit: self.text(TextKey::CacheHit),
            },
        );
        let muted = cx.theme().muted_foreground;
        // Collect the figures that exist, then join them with the separator so
        // a dropped figure never leaves a dangling `·`.
        let mut figures: Vec<String> = Vec::new();
        if let Some(turns) = line.turns {
            figures.push(turns);
        }
        if let Some(rate) = line.rate {
            figures.push(rate);
        }
        if let Some(tokens) = line.tokens {
            figures.push(tokens);
        }
        if let Some(cache) = line.cache {
            figures.push(cache);
        }
        if let Some(context) = line.context {
            figures.push(format!("{} {context}", self.text(TextKey::ContextMeter)));
        }
        let mut children: Vec<AnyElement> = Vec::new();
        for (position, figure) in figures.into_iter().enumerate() {
            if position > 0 {
                children.push(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child("·")
                        .into_any_element(),
                );
            }
            children.push(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .font_family("Lilex")
                    .text_color(muted)
                    .child(figure)
                    .into_any_element(),
            );
        }
        h_flex()
            .id("trajectory-stats")
            .debug_selector(|| "trajectory-stats".into())
            .w_full()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .pt(px(2.))
            .children(children)
            .into_any_element()
    }

    /// The selected record's detail panel.
    ///
    /// Collapsed until a record is selected, so the ledger keeps the full
    /// width while the reader is scanning rather than inspecting. Once open it
    /// is tabbed: each tab answers one question about the record instead of
    /// stacking every body into a single column.
    fn render_trajectory_detail(
        &self,
        view: &TrajectoryView,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let record = self.trajectory_selected_record(view)?;
        let row_id = record.row;
        let language = self.snapshot.language;

        // A labelled value row: `label        value` on one line, the way the
        // reference panel reads.
        let attribute = |label: &'static str, value: Option<String>, cx: &mut Context<Self>| {
            h_flex()
                .w_full()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(88.))
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .font_family("Lilex")
                        .child(
                            value.unwrap_or_else(|| self.text(TextKey::DetailNoValue).to_string()),
                        ),
                )
        };
        // A monospace body block: payloads, results, schema.
        let block = |id: &'static str, body: Option<String>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w_full()
                .rounded(px(6.))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().group_box)
                .p(px(8.))
                .text_size(px(11.))
                .font_family("Lilex")
                .child(body.unwrap_or_else(|| self.text(TextKey::DetailNoValue).to_string()))
        };

        let timing = record.duration_ms.map(trajectory_page::format_duration);
        let header = h_flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .rounded(px(4.))
                    .bg(cx.theme().accent)
                    .px(px(6.))
                    .py(px(1.))
                    .text_size(px(9.5))
                    .font_semibold()
                    .font_family("Lilex")
                    .text_color(cx.theme().link)
                    .child(record.kind.label()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(10.))
                    .font_family("Lilex")
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} {} · {} {}",
                        self.text(TextKey::TurnPrefix),
                        record.turn,
                        i18n::text(language, TextKey::Steps),
                        record.step
                    )),
            )
            .child(
                div()
                    .id("trajectory-detail-close")
                    .debug_selector(|| "trajectory-detail-close".into())
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .aria_label(self.text(TextKey::DetailClose))
                    .cursor_pointer()
                    .rounded(px(4.))
                    .px(px(6.))
                    .py(px(2.))
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .hover(|style| style.bg(cx.theme().muted))
                    .child("×")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.trajectory_selected = None;
                        this.trajectory_selected_index = None;
                        cx.notify();
                    })),
            );

        let active = self.trajectory_tab;
        let tabs = h_flex()
            .id("trajectory-detail-tabs")
            .debug_selector(|| "trajectory-detail-tabs".into())
            .role(Role::TabList)
            .w_full()
            .gap(px(2.))
            .border_b_1()
            .border_color(cx.theme().border)
            .children(TrajectoryDetailTab::ALL.into_iter().map(|tab| {
                let selected = tab == active;
                div()
                    .id(SharedString::from(tab.selector()))
                    .debug_selector(move || tab.selector().to_string())
                    .role(Role::Tab)
                    .focusable()
                    .tab_stop(true)
                    .aria_label(self.text(tab.label_key()))
                    .cursor_pointer()
                    .border_b_2()
                    .when(selected, |tab| tab.border_color(cx.theme().link))
                    .when(!selected, |tab| tab.border_color(gpui::transparent_black()))
                    .px(px(7.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .when(selected, |tab| tab.text_color(cx.theme().link))
                    .when(!selected, |tab| tab.text_color(cx.theme().muted_foreground))
                    .hover(|style| style.text_color(cx.theme().foreground))
                    .child(self.text(tab.label_key()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.trajectory_tab = tab;
                        cx.notify();
                    }))
            }));

        let body: AnyElement = match active {
            TrajectoryDetailTab::Summary => v_flex()
                .id("trajectory-tab-body-summary")
                .debug_selector(|| "trajectory-tab-body-summary".into())
                .w_full()
                .gap(px(7.))
                .child(attribute(
                    self.text(TextKey::DetailHierarchy),
                    Some(format!(
                        "{} {} · {} {}",
                        self.text(TextKey::TurnPrefix),
                        record.turn,
                        self.text(TextKey::RecordPrefix),
                        record.index
                    )),
                    cx,
                ))
                .child(attribute(
                    self.text(TextKey::RecordStatus),
                    Some(record.status_label().to_string()),
                    cx,
                ))
                .child(attribute(
                    self.text(TextKey::DetailStarted),
                    record.at.as_ref().map(|at| at.to_string()),
                    cx,
                ))
                .child(attribute(
                    self.text(TextKey::DetailDuration),
                    timing.clone(),
                    cx,
                ))
                .into_any_element(),
            TrajectoryDetailTab::Payload => v_flex()
                .id("trajectory-tab-body-payload")
                .debug_selector(|| "trajectory-tab-body-payload".into())
                .w_full()
                .gap(px(7.))
                .child(block(
                    "trajectory-field-Payload",
                    record.input.as_ref().map(|body| body.to_string()),
                    cx,
                ))
                .into_any_element(),
            TrajectoryDetailTab::Result => v_flex()
                .id("trajectory-tab-body-result")
                .debug_selector(|| "trajectory-tab-body-result".into())
                .w_full()
                .gap(px(7.))
                .child(block(
                    "trajectory-field-Result",
                    record.output.as_ref().map(|body| body.to_string()),
                    cx,
                ))
                .into_any_element(),
            TrajectoryDetailTab::Schema => v_flex()
                .id("trajectory-tab-body-schema")
                .debug_selector(|| "trajectory-tab-body-schema".into())
                .w_full()
                .gap(px(7.))
                .child(attribute(
                    self.text(TextKey::DetailSchema),
                    record.tool_name.as_ref().map(|name| name.to_string()),
                    cx,
                ))
                .when_some(
                    record.tool_id.as_ref().map(|id| id.to_string()),
                    |body, id| body.child(attribute("ID", Some(id), cx)),
                )
                .into_any_element(),
            TrajectoryDetailTab::Timing => v_flex()
                .id("trajectory-tab-body-timing")
                .debug_selector(|| "trajectory-tab-body-timing".into())
                .w_full()
                .gap(px(7.))
                .child(attribute(
                    self.text(TextKey::DetailDuration),
                    Some(match timing {
                        Some(timing) => {
                            format!("{} · {}", timing, self.text(TextKey::Measured))
                        }
                        None => self.text(TextKey::NoRecordedDuration).to_string(),
                    }),
                    cx,
                ))
                .into_any_element(),
        };

        v_flex()
            .id("trajectory-detail")
            .debug_selector(|| "trajectory-detail".into())
            .role(Role::Complementary)
            .aria_label(self.text(TextKey::RecordDetail))
            .w(px(320.))
            .flex_none()
            .h_full()
            .min_h_0()
            .gap(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .p(px(14.))
            .child(header)
            .child(tabs)
            .child(
                v_flex()
                    .id("trajectory-detail-body")
                    .debug_selector(|| "trajectory-detail-body".into())
                    .flex_1()
                    .min_h_0()
                    // A tool result can run to hundreds of lines. Without a
                    // scroll owner the body lets that content set its height,
                    // and the text paints straight past the column over the
                    // ledger. Tracking the scroll bounds it to the column.
                    .overflow_y_scroll()
                    .track_scroll(&self.trajectory_detail_scroll)
                    .gap(px(8.))
                    .child(
                        div()
                            .w_full()
                            .text_size(px(12.))
                            .font_semibold()
                            .child(record.text.to_string()),
                    )
                    .child(body),
            )
            .child(
                div()
                    .id("trajectory-detail-open")
                    .debug_selector(|| "trajectory-detail-open".into())
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .aria_label(self.text(TextKey::OpenReader))
                    .cursor_pointer()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(cx.theme().link)
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(self.text(TextKey::OpenInChat))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.open_transcript_row_reader(row_id, cx);
                    })),
            )
            .into_any_element()
            .into()
    }

    fn render_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.snapshot.clone();
        let transcript = snapshot.transcript.clone();
        if transcript.is_empty() {
            return v_flex()
                .id("empty-transcript")
                .size_full()
                .items_center()
                .justify_center()
                .p_8()
                .child(
                    v_flex()
                        .w(px(520.))
                        .max_w_full()
                        .gap_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .p_6()
                        .shadow_sm()
                        .child(
                            div()
                                .text_xl()
                                .font_semibold()
                                .child(self.text(TextKey::EmptySessionTitle)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.text(TextKey::EmptySessionBody)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(composer_insert_button(
                                    "empty-slash",
                                    "/",
                                    self.text(TextKey::InsertCommand),
                                    "/",
                                    cx,
                                ))
                                .child(composer_insert_button(
                                    "empty-mention",
                                    "@",
                                    self.text(TextKey::MentionFile),
                                    "@",
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.text(TextKey::EmptySessionHint)),
                        ),
                )
                .into_any_element();
        }
        let session_id = self.transcript_session_id();
        self.touch_transcript_session_cache(&session_id);
        let assistant_selections =
            self.sync_assistant_selections(&transcript, "main", &session_id, window, cx);
        let row_views = self.sync_transcript_row_views(
            &transcript,
            &session_id,
            snapshot.clone(),
            &assistant_selections,
            cx,
        );
        let new_activity = self.new_activity;
        let wheel_enabled = self.overlay.is_none();
        let answers_only = self.answers_only;
        let live_label = self.live_activity_label();
        let scroller_changed = self.transcript_scroller.update(cx, |view, cx| {
            view.set_state(
                TranscriptViewState {
                    rows: row_views,
                    new_activity,
                    wheel_enabled,
                    answers_only,
                    live_label,
                },
                &session_id,
                &snapshot,
                cx,
            )
        });
        if scroller_changed {
            cx.notify();
        }
        let run_text = transcript
            .iter()
            .find_map(|row| match &row.body {
                RowBody::User(user) => Some(strip_leading_url(&user.text)),
                _ => None,
            })
            .unwrap_or_else(|| self.snapshot.session.title.to_string());
        let (title, subtitle) = split_run_text(run_text);
        let session_label = self
            .snapshot
            .session
            .id
            .as_ref()
            .map(|id| {
                let raw = id.0.to_string();
                let short = raw.rsplit('-').next().unwrap_or(&raw);
                short[..short.len().min(4)].to_uppercase()
            })
            .unwrap_or_else(|| "—".to_string());
        let elapsed = self
            .snapshot
            .runtime
            .turn_started_at
            .or(self.snapshot.runtime.run_started_at)
            .map(format_elapsed)
            .unwrap_or_else(|| "0s".to_string());
        let turns = self.snapshot.runtime.turns_taken;
        let running = matches!(
            self.snapshot.session.phase,
            SessionPhase::Running
                | SessionPhase::Starting
                | SessionPhase::Opening
                | SessionPhase::Switching
        );
        let column = v_flex()
            .id("transcript-column")
            .flex_1()
            .min_w_0()
            .h_full()
            .w_full()
            .max_w(px(900.))
            .mx_auto()
            .child(
                v_flex()
                    .flex_none()
                    .gap_1()
                    .px_0()
                    .pt(px(14.))
                    .pb(px(18.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .debug_selector(|| "transcript-run-meta".into())
                            .gap_2()
                            .items_center()
                            .text_size(px(10.))
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(div().size(px(6.)).rounded_full().bg(if running {
                                cx.theme().success
                            } else {
                                cx.theme().muted_foreground
                            }))
                            .child(div().font_family("Lilex").child(format!(
                                "{} {session_label}",
                                self.text(TextKey::RunLabel).to_uppercase()
                            )))
                            .child(div().h(px(1.)).w(px(16.)).bg(cx.theme().border))
                            .child(div().font_family("Lilex").child(format!(
                                "{elapsed} {}",
                                self.text(TextKey::ElapsedLabel).to_uppercase()
                            )))
                            .child(div().font_family("Lilex").child(format!(
                                "{} {turns}",
                                self.text(TextKey::TurnLabel).to_uppercase()
                            ))),
                    )
                    .child({
                        // Keep the run title subordinate to the conversation:
                        // compact at `clamp(18px, 1.6vw, 23px)`.
                        let cjk_title = is_cjk_title(self.snapshot.language, &title);
                        let headline = (f32::from(window.viewport_size().width) * 0.016)
                            .clamp(18., 23.)
                            * self.snapshot.text_scale;
                        div()
                            .max_w(px(900.))
                            .font_family("SF Pro Display")
                            .font_semibold()
                            .when(cjk_title, |title| {
                                title
                                    .max_w(px(840. * self.snapshot.text_scale))
                                    .text_size(px(headline))
                                    .line_height(px(headline * 1.15))
                            })
                            .when(!cjk_title, |title| {
                                title
                                    .text_size(px(headline))
                                    .line_height(px(headline * 1.2))
                            })
                            .child(title)
                    })
                    .when_some(subtitle, |column, subtitle| {
                        column.child(
                            div()
                                .max_w(px(900.))
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .text_color(cx.theme().muted_foreground)
                                .child(subtitle),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.transcript_scroller.clone()),
            );
        // A session switch replaces the whole row set in one frame; a short
        // settle keeps that swap from reading as a flash.
        if cx.reduce_motion() {
            column.into_any_element()
        } else {
            let epoch = self.snapshot.session.epoch.0;
            column
                .with_animation(
                    ("session-swap", epoch),
                    Animation::new(Duration::from_millis(180)),
                    |column, delta| column.opacity(delta),
                )
                .into_any_element()
        }
    }

    fn sync_transcript_row_views(
        &mut self,
        transcript: &TranscriptView,
        session_id: &SessionId,
        snapshot: Arc<DesktopSnapshot>,
        assistant_selections: &HashMap<(String, SessionId, RowId), Entity<SelectionActions>>,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<TranscriptRowView>> {
        let live = transcript.iter().map(|row| row.id).collect::<HashSet<_>>();
        self.transcript_rows.retain(|(cached_session, row_id), _| {
            cached_session != session_id || live.contains(row_id)
        });
        let mut views = Vec::with_capacity(transcript.len());
        for row in transcript.iter() {
            let key = (session_id.clone(), row.id);
            let selection = assistant_selections
                .get(&("main".to_string(), session_id.clone(), row.id))
                .cloned();
            let thinking_open = self
                .expanded_thinking
                .contains(&(session_id.clone(), row.id));
            let cached_view = self.transcript_rows.get(&key).cloned();
            let needs_update = self.pending_row_updates.contains(&row.id)
                || cached_view
                    .as_ref()
                    .is_none_or(|view| !Arc::ptr_eq(&view.read(cx).row, row));
            let view = if let Some(view) = cached_view {
                let cached = view.read(cx);
                let snapshot_changed = !Arc::ptr_eq(&cached.snapshot, &snapshot);
                let context_changed = cached.snapshot.appearance != snapshot.appearance
                    || cached.snapshot.language != snapshot.language
                    || cached.snapshot.show_hook_output != snapshot.show_hook_output
                    || (needs_update && cached.snapshot.session.epoch != snapshot.session.epoch)
                    || cached.thinking_open != thinking_open;
                if needs_update || context_changed || snapshot_changed {
                    view.update(cx, |view, cx| {
                        view.row = row.clone();
                        view.snapshot = snapshot.clone();
                        view.assistant_selection = selection;
                        view.thinking_open = thinking_open;
                        if needs_update || context_changed {
                            cx.notify();
                        }
                    });
                }
                view
            } else {
                let root = cx.weak_entity();
                let view = cx.new(|cx| TranscriptRowView {
                    row: row.clone(),
                    snapshot: snapshot.clone(),
                    root,
                    assistant_selection: selection,
                    thinking_open,
                    gate_focus: cx.focus_handle(),
                });
                self.transcript_rows.insert(key, view.clone());
                view
            };
            views.push(view);
        }
        self.pending_row_updates.clear();
        views
    }

    fn render_inspector(
        &mut self,
        width: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Labels are bare: the counts already sit in the sticky strip and the
        // rail on the same screen, and duplicating them costs the width the
        // fifth tab needs.
        let tabs = [
            (InspectorTab::Run, self.text(TextKey::Run)),
            (InspectorTab::Goals, self.text(TextKey::Goals)),
            (InspectorTab::Subagents, self.text(TextKey::Subagents)),
            (InspectorTab::Context, self.text(TextKey::Context)),
            // Appended last so the digit shortcuts keep their existing
            // meanings: Ctrl+4 still opens Context, Ctrl+5 opens Background.
            (InspectorTab::Background, self.text(TextKey::Background)),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (tab, label))| {
            let selected = self.snapshot.inspector.tab == tab;
            div()
                .id(SharedString::from(format!("inspector-tab-{index}")))
                .debug_selector(move || format!("inspector-tab-{index}"))
                .role(Role::Tab)
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                .aria_selected(selected)
                .aria_label(label)
                .flex_none()
                .cursor_pointer()
                .px(px(6.))
                .pt(px(8.))
                .pb(px(7.))
                .text_size(px(11.))
                .when(selected, |tab| {
                    tab.text_color(cx.theme().foreground)
                        .border_b_2()
                        .border_color(cx.theme().primary)
                })
                .when(!selected, |tab| tab.text_color(cx.theme().muted_foreground))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.select_inspector(tab, cx);
                }))
        })
        .collect::<Vec<_>>();
        let tab_buttons = tabs;
        let content = match self.snapshot.inspector.tab {
            InspectorTab::Run => self.render_run_inspector(cx),
            InspectorTab::Goals => self.render_goals_inspector(cx),
            InspectorTab::Subagents => self.render_subagents_inspector(cx),
            InspectorTab::Context => self.render_context_inspector(cx),
            InspectorTab::Background => self.render_background_inspector(cx),
        };
        let tab_index = match self.snapshot.inspector.tab {
            InspectorTab::Run => 0,
            InspectorTab::Goals => 1,
            InspectorTab::Subagents => 2,
            InspectorTab::Context => 3,
            InspectorTab::Background => 4,
        };
        let direct_motion = crate::ui::shell::motion_policy(self.snapshot.motion)
            .direct_state_changes
            || cx.reduce_motion();
        let content = if direct_motion {
            div().child(content).into_any_element()
        } else {
            div()
                .child(content)
                .with_animation(
                    ("inspector-tab-content", tab_index as u64),
                    Animation::new(Duration::from_millis(160)),
                    |body, delta| body.opacity(delta).relative().left(px((1.0 - delta) * 6.0)),
                )
                .into_any_element()
        };
        v_flex()
            .id("run-inspector")
            .debug_selector(|| "run-inspector".into())
            .role(Role::Complementary)
            .aria_label(self.text(TextKey::RunInspector))
            .w(width)
            .h_full()
            .p(px(16.))
            .gap(px(14.))
            .font_family(theme::native_font_family())
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => "INSPECTOR",
                                tact_catalog::CatalogLocale::Zh => "检查器",
                            }),
                    )
                    .child(div().text_size(px(24.)).font_semibold().child(
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => "Run map",
                            tact_catalog::CatalogLocale::Zh => "运行图",
                        },
                    )),
            )
            .child(
                h_flex()
                    .id("inspector-tabs")
                    .role(Role::TabList)
                    .aria_label(self.text(TextKey::RunInspector))
                    .w_full()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    // A fifth tab does not always fit 318px, and text scale can
                    // widen every label. Scroll instead of clipping one away.
                    .overflow_x_scroll()
                    .children(tab_buttons),
            )
            .child(
                div()
                    .id("inspector-content")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .min_w_0()
                    // The vertical scrollbar overlays the content edge; keep
                    // right-aligned values clear of it.
                    .pr(px(14.))
                    .overflow_y_scrollbar()
                    .child(content),
            )
    }

    fn kicker(text: impl Into<SharedString>, cx: &mut Context<DesktopRoot>) -> impl IntoElement {
        div()
            .text_size(px(10.))
            .font_family("Lilex")
            .text_color(cx.theme().muted_foreground)
            .child(text.into())
    }

    fn activity_line(
        text: String,
        accent: bool,
        cx: &mut Context<DesktopRoot>,
    ) -> impl IntoElement {
        div()
            .text_size(px(11.))
            .font_family("Lilex")
            .when(accent, |line| line.text_color(cx.theme().primary))
            .when(!accent, |line| line.text_color(cx.theme().muted_foreground))
            .truncate()
            .child(text)
    }

    fn context_gauge(ratio: f32, percent: u32, cx: &mut Context<DesktopRoot>) -> AnyElement {
        let track = cx.theme().muted;
        let fill = cx.theme().primary;
        let label = percent.to_string();
        div()
            .relative()
            .flex_none()
            .size(px(68.))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let center = bounds.center();
                        let radius = bounds.size.width.min(bounds.size.height) / 2. - px(4.);
                        let mut ring = PathBuilder::stroke(px(4.));
                        ring.move_to(center + point(radius, px(0.)));
                        ring.arc_to(
                            point(radius, radius),
                            px(0.),
                            false,
                            true,
                            center + point(-radius, px(0.)),
                        );
                        ring.arc_to(
                            point(radius, radius),
                            px(0.),
                            false,
                            true,
                            center + point(radius, px(0.)),
                        );
                        if let Ok(path) = ring.build() {
                            window.paint_path(path, track);
                        }
                        if ratio > 0.0 {
                            let start = -std::f32::consts::FRAC_PI_2;
                            let end = start + std::f32::consts::TAU * ratio;
                            let from = center + point(radius * start.cos(), radius * start.sin());
                            let to = center + point(radius * end.cos(), radius * end.sin());
                            let mut arc = PathBuilder::stroke(px(5.));
                            arc.move_to(from);
                            arc.arc_to(point(radius, radius), px(0.), ratio > 0.5, true, to);
                            if let Ok(path) = arc.build() {
                                window.paint_path(path, fill);
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(15.))
                    .font_semibold()
                    .child(label),
            )
            .into_any_element()
    }

    fn render_run_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let runtime = &self.snapshot.runtime;
        let phase = run_phase_label(runtime.current_phase, self.snapshot.language);
        let total = runtime.usage.total_tokens;
        let window = self
            .snapshot
            .runtime
            .usage
            .context_window
            .filter(|window| *window > 0)
            .unwrap_or(200_000);
        let ratio = (total as f32 / window as f32).clamp(0.0, 1.0);
        let percent = (ratio * 100.0).round() as u32;
        let pending = self.snapshot.pending_gates.len();
        let title = self.snapshot.session.title.to_string();
        let tasks = &self.snapshot.inspector.tasks;
        let current = tasks.iter().position(|task| {
            matches!(
                task.status,
                tact_desktop_core::TaskStatusSnapshot::InProgress
            )
        });
        let agents = self.snapshot.inspector.subagents.len();
        let background = &self.snapshot.inspector.background;

        let activity_rows = background
            .iter()
            .take(2)
            .map(|task| Self::activity_line(task.command.to_string(), false, cx).into_any_element())
            .collect::<Vec<_>>();

        v_flex()
            .id("run-inspector-content")
            .role(Role::Region)
            .aria_label(self.text(TextKey::Run))
            .w_full()
            .min_w_0()
            .gap(px(14.))
            .child(
                v_flex()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .p(px(14.))
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .items_center()
                            .child(Self::context_gauge(ratio, percent, cx))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(3.))
                                    .child(Self::kicker(
                                        match self.snapshot.language {
                                            tact_catalog::CatalogLocale::En => "ACTIVE RUN",
                                            tact_catalog::CatalogLocale::Zh => "活动运行",
                                        },
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_semibold()
                                            .truncate()
                                            .child(title),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(px(6.))
                                            .items_center()
                                            .child(div().size(px(6.)).rounded_full().bg(
                                                if pending > 0 {
                                                    cx.theme().warning
                                                } else {
                                                    cx.theme().success
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(if pending > 0 {
                                                        match self.snapshot.language {
                                                            tact_catalog::CatalogLocale::En => {
                                                                "Waiting on permission"
                                                            }
                                                            tact_catalog::CatalogLocale::Zh => {
                                                                "等待权限"
                                                            }
                                                        }
                                                    } else {
                                                        phase
                                                    }),
                                            ),
                                    ),
                            ),
                    ),
            )
            .when_some(current, |column, index| {
                let task = &tasks[index];
                column.child(
                    v_flex()
                        .gap(px(6.))
                        .child(
                            h_flex()
                                .justify_between()
                                .child(Self::kicker(
                                    match self.snapshot.language {
                                        tact_catalog::CatalogLocale::En => "CURRENT STEP",
                                        tact_catalog::CatalogLocale::Zh => "当前步骤",
                                    },
                                    cx,
                                ))
                                .child(Self::kicker(
                                    format!("{:02} / {:02}", index + 1, tasks.len()),
                                    cx,
                                )),
                        )
                        .child(
                            v_flex()
                                .rounded(px(6.))
                                .border_l(px(2.))
                                .border_color(cx.theme().primary)
                                .bg(cx.theme().accent)
                                .p(px(10.))
                                .gap(px(3.))
                                .child(
                                    h_flex()
                                        .gap(px(8.))
                                        .items_center()
                                        .child(
                                            div()
                                                .flex_none()
                                                .size(px(22.))
                                                .rounded_full()
                                                .border_1()
                                                .border_color(cx.theme().primary)
                                                .text_size(px(10.))
                                                .font_family("Lilex")
                                                .text_color(cx.theme().link)
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(format!("{:02}", index + 1)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(13.))
                                                .font_semibold()
                                                .child(task.subject.to_string()),
                                        ),
                                )
                                .child(
                                    div()
                                        .pl(px(30.))
                                        .text_size(px(11.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(task.owner.to_string()),
                                ),
                        ),
                )
            })
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(Self::kicker(
                                match self.snapshot.language {
                                    tact_catalog::CatalogLocale::En => "THREAD SIGNALS",
                                    tact_catalog::CatalogLocale::Zh => "线程信号",
                                },
                                cx,
                            ))
                            .child(Self::kicker("LIVE", cx)),
                    )
                    .child(signal_row(
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => "Main agent",
                            tact_catalog::CatalogLocale::Zh => "主代理",
                        },
                        phase.to_string(),
                        cx.theme().success,
                        cx,
                    ))
                    .child(signal_row(
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => "Permission gate",
                            tact_catalog::CatalogLocale::Zh => "权限门",
                        },
                        if pending > 0 {
                            match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => "waiting",
                                tact_catalog::CatalogLocale::Zh => "等待中",
                            }
                            .to_string()
                        } else {
                            match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => "clear",
                                tact_catalog::CatalogLocale::Zh => "无",
                            }
                            .to_string()
                        },
                        if pending > 0 {
                            cx.theme().warning
                        } else {
                            cx.theme().success
                        },
                        cx,
                    ))
                    .child(signal_row(
                        self.text(TextKey::Subagents),
                        agents.to_string(),
                        cx.theme().info,
                        cx,
                    ))
                    .child(signal_row(
                        self.text(TextKey::Background),
                        background.len().to_string(),
                        cx.theme().muted_foreground,
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(Self::kicker(
                                match self.snapshot.language {
                                    tact_catalog::CatalogLocale::En => "LATEST ACTIVITY",
                                    tact_catalog::CatalogLocale::Zh => "最近活动",
                                },
                                cx,
                            ))
                            .child(Self::kicker("LIVE", cx)),
                    )
                    .when_some(runtime.latest_tool_detail.clone(), |column, tool| {
                        let label = truncate_display_text(&tool.label, 34);
                        let status = match tool.status {
                            RowStatus::Pending => ToolStatus::Pending,
                            RowStatus::Running => ToolStatus::Running,
                            RowStatus::Success | RowStatus::Resolved => ToolStatus::Success,
                            RowStatus::Failed => ToolStatus::Failed,
                            RowStatus::Cancelled => ToolStatus::Cancelled,
                            RowStatus::Unknown => ToolStatus::Unknown,
                        };
                        let status_label = match tool.status {
                            RowStatus::Pending => self.text(TextKey::Pending),
                            RowStatus::Running => self.text(TextKey::Running),
                            RowStatus::Success | RowStatus::Resolved => {
                                self.text(TextKey::Completed)
                            }
                            RowStatus::Failed => self.text(TextKey::Failed),
                            RowStatus::Cancelled => self.text(TextKey::Cancelled),
                            RowStatus::Unknown => self.text(TextKey::Unknown),
                        };
                        let mut chip = ToolChip::new(tool.tool_id.clone(), label)
                            .status(status)
                            .status_label(status_label);
                        if let Some(duration_ms) = tool.duration_ms {
                            chip =
                                chip.detail(crate::ui::background::format_duration_ms(duration_ms));
                        }
                        if let (Some(group), Some(row)) = (tool.group, tool.row) {
                            let weak = cx.weak_entity();
                            chip = chip.on_event(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    let group_row =
                                        this.snapshot.transcript.iter().find_map(|candidate| {
                                            let RowBody::ToolGroup(candidate_group) =
                                                &candidate.body
                                            else {
                                                return None;
                                            };
                                            if candidate.id != group {
                                                return None;
                                            }
                                            let expanded =
                                                candidate_group.items.iter().find_map(|item| {
                                                    match item {
                                                        ToolGroupItem::Tool(tool)
                                                            if tool.id == row =>
                                                        {
                                                            Some(tool.expanded)
                                                        }
                                                        _ => None,
                                                    }
                                                });
                                            Some((candidate_group.collapsed, expanded))
                                        });
                                    if let Some((collapsed, expanded)) = group_row {
                                        if expanded != Some(true) {
                                            this.dispatch(DesktopIntent::ToggleTool {
                                                group,
                                                tool: row,
                                            });
                                        }
                                        if collapsed {
                                            this.dispatch(DesktopIntent::ToggleGroup { id: group });
                                        }
                                    }
                                    this.remember_focus(window, cx);
                                    this.open_transcript_reader(ReaderRef::ToolOutput(row), cx);
                                    let inline = crate::ui::layout::layout(
                                        f32::from(window.viewport_size().width),
                                        this.snapshot.text_scale,
                                        this.left_visible,
                                        true,
                                    )
                                    .inspector_inline;
                                    if !inline {
                                        this.right_visible = false;
                                        this.restore_focus(window, cx);
                                    }
                                    cx.notify();
                                });
                            });
                        }
                        column.child(
                            h_flex().w_full().min_w_0().items_center().child(
                                h_flex()
                                    .flex_none()
                                    .max_w_full()
                                    .min_w_0()
                                    .items_center()
                                    .debug_selector(|| "inspector-latest-tool".into())
                                    .child(chip),
                            ),
                        )
                    })
                    .children(activity_rows),
            )
            .child(
                div()
                    .pt(px(4.))
                    .text_size(px(10.))
                    .font_family("Lilex")
                    .text_color(cx.theme().muted_foreground)
                    .child(match self.snapshot.language {
                        tact_catalog::CatalogLocale::En => {
                            "Ctrl 1-5 · Inspect without losing the run"
                        }
                        tact_catalog::CatalogLocale::Zh => "Ctrl 1-5 · 检查而不离开运行",
                    }),
            )
            .into_any_element()
    }

    /// The Background inspector: every background task this session started,
    /// running ones first.
    ///
    /// Ordering mirrors the sticky task strip — what is still moving leads —
    /// and within a group the most recently started task comes first, so the
    /// top row is always the newest activity.
    fn render_background_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let tasks = &self.snapshot.inspector.background;
        let running = tasks
            .iter()
            .filter(|task| task.status == tact_desktop_core::BackgroundTaskStatusSnapshot::Running)
            .count();
        let mut ordered: Vec<&tact_desktop_core::BackgroundTaskView> = tasks.iter().collect();
        ordered.sort_by_key(|task| {
            let running = task.status == tact_desktop_core::BackgroundTaskStatusSnapshot::Running;
            (
                std::cmp::Reverse(running),
                std::cmp::Reverse(task.started_at),
            )
        });
        let rows = self.background_task_rows_for(&ordered, cx);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .id("background-inspector-summary")
                    .debug_selector(|| "background-inspector-summary".into())
                    .justify_between()
                    .child(section_label(self.text(TextKey::Background), cx))
                    .child(
                        div()
                            .id("background-inspector-running")
                            .debug_selector(|| "background-inspector-running".into())
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{running}/{}", tasks.len())),
                    ),
            )
            .when(tasks.is_empty(), |column| {
                column.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.text(TextKey::NoItems)),
                )
            })
            .children(rows)
            .into_any_element()
    }

    fn background_task_rows_for(
        &self,
        tasks: &[&tact_desktop_core::BackgroundTaskView],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        tasks
            .iter()
            .map(|task| {
                let meta = crate::ui::background::meta_label(
                    task,
                    chrono::Utc::now().timestamp_millis(),
                    background_status_label(task.status, self.snapshot.language),
                );
                v_flex()
                    .id(SharedString::from(format!(
                        "inspector-background-{}",
                        task.id
                    )))
                    .debug_selector({
                        let selector = format!("background-row-{}", task.id);
                        move || selector.clone()
                    })
                    .role(Role::Status)
                    .aria_label(format!("{meta} · {}", task.command))
                    .gap_1()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(task.command.to_string()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().text_xs().child(meta))
                            .child(
                                div()
                                    .id(SharedString::from(format!("open-background-{}", task.id)))
                                    .debug_selector({
                                        let selector = format!("open-background-{}", task.id);
                                        move || selector.clone()
                                    })
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(self.text(TextKey::OpenBackgroundOutput))
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(cx.theme().link)
                                    .child(self.text(TextKey::Open))
                                    .on_click(cx.listener({
                                        let id = task.id.to_string();
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.remember_focus(window, cx);
                                            this.open_transcript_reader(
                                                ReaderRef::Background(id.clone()),
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("stop-{}", task.id)))
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(self.text(TextKey::StopBackground))
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(self.text(TextKey::Stop))
                                    .on_click(cx.listener({
                                        let id = task.id.to_string();
                                        move |this, _: &ClickEvent, _, _| {
                                            this.dispatch(DesktopIntent::CancelBackgroundTask {
                                                task_id: id.clone(),
                                            });
                                        }
                                    })),
                            ),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn render_goals_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let create_goal = self.text(TextKey::CreateGoal);
        let objective_label = argument_label("objective", self.snapshot.language);
        let goals = self
            .snapshot
            .inspector
            .goals
            .iter()
            .map(|goal| {
                let snapshot = goal.snapshot.clone();
                let summary = snapshot
                    .as_ref()
                    .map(|goal| {
                        format!(
                            "{} · {}",
                            goal.objective
                                .lines()
                                .next()
                                .unwrap_or(self.text(TextKey::Goals)),
                            goal_status_label(goal.status, self.snapshot.language)
                        )
                    })
                    .unwrap_or_else(|| self.text(TextKey::NoGoalSnapshot).into());
                let Some(snapshot) = snapshot else {
                    return div()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().border)
                        .p_2()
                        .text_sm()
                        .child(summary)
                        .into_any_element();
                };
                let goal_id = snapshot.id.clone();
                let steer_id = goal_id.clone();
                let accept_id = goal_id.clone();
                let details = [
                    (
                        self.text(TextKey::Phase),
                        goal_phase_label(snapshot.phase, self.snapshot.language).to_string(),
                    ),
                    (
                        self.text(TextKey::Criteria),
                        format!(
                            "{} / {}",
                            snapshot.acceptance.passed, snapshot.acceptance.required
                        ),
                    ),
                    (
                        self.text(TextKey::Acceptance),
                        format!(
                            "{} + {} - {}",
                            snapshot.acceptance.pending,
                            snapshot.acceptance.passed,
                            snapshot.acceptance.rejected
                        ),
                    ),
                    (
                        self.text(TextKey::Tasks),
                        format!("{}/{}", snapshot.task_completed, snapshot.task_total),
                    ),
                    (
                        self.text(TextKey::RunBudget),
                        snapshot
                            .run_budget
                            .as_ref()
                            .map(|budget| match self.snapshot.language {
                                tact_catalog::CatalogLocale::Zh => format!(
                                    "{}/{} 轮 · {}/{} 毫秒",
                                    budget.turns_used,
                                    budget.turn_limit,
                                    budget.elapsed_ms,
                                    budget.wall_clock_limit_ms
                                ),
                                tact_catalog::CatalogLocale::En => format!(
                                    "{}/{} turns · {}/{} ms",
                                    budget.turns_used,
                                    budget.turn_limit,
                                    budget.elapsed_ms,
                                    budget.wall_clock_limit_ms
                                ),
                            })
                            .unwrap_or_else(|| "—".into()),
                    ),
                    (
                        self.text(TextKey::WaitReason),
                        snapshot
                            .wait_reason
                            .as_deref()
                            .map(|reason| localized_wait_reason(reason, self.snapshot.language))
                            .unwrap_or_else(|| "—".into()),
                    ),
                ];
                let plan_steps = snapshot
                    .criteria
                    .iter()
                    .map(|criterion| {
                        PlanStep::new(criterion.id.clone(), criterion.description.clone())
                            .detail(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => {
                                    format!("{} evidence", criterion.evidence_count)
                                }
                                tact_catalog::CatalogLocale::Zh => {
                                    format!("{} 条证据", criterion.evidence_count)
                                }
                            })
                            .status(match criterion.status {
                                tact_desktop_core::GoalCriterionStatusSnapshot::Pending => {
                                    PlanStepStatus::Pending
                                }
                                tact_desktop_core::GoalCriterionStatusSnapshot::Passed => {
                                    PlanStepStatus::Done
                                }
                                tact_desktop_core::GoalCriterionStatusSnapshot::Rejected => {
                                    PlanStepStatus::Failed
                                }
                            })
                    })
                    .collect::<Vec<_>>();
                let plan_state = match snapshot.status {
                    tact_desktop_core::GoalStatusSnapshot::Draft
                    | tact_desktop_core::GoalStatusSnapshot::Queued => PlanState::Proposed,
                    tact_desktop_core::GoalStatusSnapshot::Running
                    | tact_desktop_core::GoalStatusSnapshot::WaitingUser
                    | tact_desktop_core::GoalStatusSnapshot::WaitingPermission
                    | tact_desktop_core::GoalStatusSnapshot::Verifying => PlanState::Running,
                    tact_desktop_core::GoalStatusSnapshot::Paused => PlanState::Approved,
                    tact_desktop_core::GoalStatusSnapshot::Succeeded => PlanState::Completed,
                    tact_desktop_core::GoalStatusSnapshot::Failed => PlanState::Failed,
                    tact_desktop_core::GoalStatusSnapshot::Cancelled => PlanState::Rejected,
                };
                let mut actions = Vec::new();
                if matches!(
                    snapshot.status,
                    tact_desktop_core::GoalStatusSnapshot::Running
                        | tact_desktop_core::GoalStatusSnapshot::Queued
                        | tact_desktop_core::GoalStatusSnapshot::WaitingUser
                        | tact_desktop_core::GoalStatusSnapshot::WaitingPermission
                        | tact_desktop_core::GoalStatusSnapshot::Verifying
                ) {
                    actions.push(
                        action_button(
                            format!("goal-pause-{goal_id}"),
                            self.text(TextKey::Pause),
                            goal_invoke("pause", &goal_id, None),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                if snapshot.status == tact_desktop_core::GoalStatusSnapshot::Paused {
                    actions.push(
                        action_button(
                            format!("goal-resume-{goal_id}"),
                            self.text(TextKey::Resume),
                            goal_invoke("resume", &goal_id, None),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                if !snapshot.status.is_terminal() {
                    actions.push(
                        action_button(
                            format!("goal-cancel-{goal_id}"),
                            self.text(TextKey::Cancel),
                            goal_invoke("cancel", &goal_id, None),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                actions.push(
                    form_action_button(
                        format!("goal-steer-{goal_id}"),
                        self.text(TextKey::Steer),
                        FormAction::SteerGoal(steer_id),
                        cx,
                    )
                    .into_any_element(),
                );
                if snapshot.status == tact_desktop_core::GoalStatusSnapshot::WaitingUser {
                    actions.push(
                        form_action_button(
                            format!("goal-accept-{goal_id}"),
                            self.text(TextKey::Accept),
                            FormAction::AcceptGoal(accept_id),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                actions.push(
                    action_button(
                        format!("goal-archive-{goal_id}"),
                        if snapshot.archived {
                            self.text(TextKey::Unarchive)
                        } else {
                            self.text(TextKey::Archive)
                        },
                        goal_invoke(
                            if snapshot.archived {
                                "unarchive"
                            } else {
                                "archive"
                            },
                            &goal_id,
                            None,
                        ),
                        cx,
                    )
                    .into_any_element(),
                );
                v_flex()
                    .id(SharedString::from(format!("goal-{}", snapshot.id)))
                    .role(Role::Status)
                    .aria_label(format!(
                        "{summary} · {}",
                        details
                            .iter()
                            .map(|(label, value)| format!("{label}: {value}"))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    ))
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_2()
                    .child(div().text_sm().child(summary))
                    .child(
                        v_flex()
                            .id(SharedString::from(format!("goal-details-{}", snapshot.id)))
                            .gap_0p5()
                            .children(details.into_iter().map(|(label, value)| {
                                div()
                                    .id(SharedString::from(format!(
                                        "goal-detail-{}-{label}",
                                        snapshot.id
                                    )))
                                    .role(Role::Status)
                                    .aria_label(format!("{label}: {value}"))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{label}: {value}"))
                            })),
                    )
                    .when(!plan_steps.is_empty(), |card| {
                        card.child(
                            PlanCard::new(
                                format!("goal-plan-{}", snapshot.id),
                                if snapshot.title.is_empty() {
                                    snapshot.objective.clone()
                                } else {
                                    snapshot.title.clone()
                                },
                            )
                            .labels(plan_card_labels(self.snapshot.language))
                            .steps(plan_steps)
                            .state(plan_state),
                        )
                    })
                    .child(h_flex().flex_wrap().gap_1().children(actions))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        v_flex()
            .gap_2()
            .child(section_label(self.text(TextKey::Goals), cx))
            .children(goals)
            .child(
                div()
                    .id("create-goal")
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(create_goal)
                    .cursor_pointer()
                    .text_sm()
                    .text_color(cx.theme().link)
                    .child(create_goal)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        this.set_form(
                            FormState {
                                title: create_goal.into(),
                                action: FormAction::CreateGoal,
                                field: Some(objective_label.into()),
                                required: true,
                                choice: None,
                            },
                            window,
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }

    fn render_subagents_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .snapshot
            .inspector
            .subagents
            .iter()
            .map(|agent| {
                let model = agent.model.as_deref().unwrap_or("—");
                let context = format!(
                    "{}/{}",
                    agent.context.total_tokens,
                    agent
                        .context
                        .context_window
                        .map(|window| window.to_string())
                        .unwrap_or_else(|| "—".into())
                );
                let last_activity = agent.last_activity.as_deref().unwrap_or("—");
                v_flex()
                    .id(SharedString::from(format!(
                        "inspector-subagent-{}",
                        agent.child_id
                    )))
                    .role(Role::Status)
                    .aria_label(format!(
                        "{} · {} · {} · {}",
                        agent.child_id,
                        subagent_status_label(agent.status, self.snapshot.language),
                        model,
                        agent.summary
                    ))
                    .gap_1()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_2()
                    .child(div().truncate().text_sm().child(agent.child_id.to_string()))
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(subagent_status_label(agent.status, self.snapshot.language))
                            .child(crate::ui::background::format_duration_ms(agent.elapsed_ms)),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                div()
                                    .truncate()
                                    .child(format!("{}: {model}", self.text(TextKey::Model))),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .child(format!("{}: {context}", self.text(TextKey::Context))),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{}: {last_activity}",
                                self.text(TextKey::LastActivity)
                            )),
                    )
                    .child(div().truncate().text_xs().child(agent.summary.to_string()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id(SharedString::from(format!("open-{}", agent.child_id)))
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(self.text(TextKey::OpenSubagentTranscript))
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(cx.theme().link)
                                    .child(self.text(TextKey::Open))
                                    .on_click(cx.listener({
                                        let child_id = agent.child_id.to_string();
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.remember_focus(window, cx);
                                            this.open_overlay(Overlay::Subagent(child_id.clone()));
                                            cx.notify();
                                        }
                                    })),
                            )
                            .when(
                                agent.status == tact_desktop_core::SubagentStatusSnapshot::Running,
                                |row| {
                                    row.child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "cancel-{}",
                                                agent.child_id
                                            )))
                                            .role(Role::Button)
                                            .focusable()
                                            .tab_stop(true)
                                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                            .aria_label(self.text(TextKey::CancelSubagent))
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(cx.theme().danger)
                                            .child(self.text(TextKey::Cancel))
                                            .on_click(cx.listener({
                                                let child_id = agent.child_id.to_string();
                                                move |this, _: &ClickEvent, _, _| {
                                                    this.dispatch(DesktopIntent::CancelSubagent {
                                                        child_id: child_id.clone(),
                                                    });
                                                }
                                            })),
                                    )
                                },
                            ),
                    )
            })
            .collect::<Vec<_>>();
        let active = self
            .snapshot
            .inspector
            .subagents
            .iter()
            .any(|agent| agent.status == tact_desktop_core::SubagentStatusSnapshot::Running);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(section_label(self.text(TextKey::Subagents), cx))
                    .when(active, |row| row.child(Orbs::new().diameter(px(24.)))),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_context_inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let usage = &self.snapshot.inspector.context;
        let evidence = self.retained_evidence();
        let weak = cx.weak_entity();
        let cards: Vec<AnyElement> = evidence
            .into_iter()
            .map(|(row_id, hook, source, snippet)| {
                let weak = weak.clone();
                ContextCard::new(
                    format!("{}-{}", if hook { "hook" } else { "tool" }, row_id.value()),
                    source,
                )
                .snippet(snippet)
                .on_event(move |event, _, cx| {
                    let ContextCardEvent::Opened { .. } = event;
                    let _ = weak.update(cx, |this, cx| {
                        if hook {
                            if let Some((title, markdown)) = this.hook_context_reader(row_id) {
                                this.open_overlay(Overlay::Reader(ReaderState::new(
                                    title, markdown,
                                )));
                            }
                        } else {
                            this.open_transcript_reader(ReaderRef::ToolOutput(row_id), cx);
                        }
                        cx.notify();
                    });
                })
                .into_any_element()
            })
            .collect();
        let context = ContextUsage::new(
            usage.total_tokens as u64,
            usage.context_window.unwrap_or(0) as u64,
        )
        .input(usage.prompt_tokens as u64)
        .output(usage.completion_tokens as u64)
        .cached(usage.cache_hit_tokens as u64)
        .reasoning(usage.reasoning_tokens as u64);
        v_flex()
            .gap_2()
            .child(section_label(self.text(TextKey::Context), cx))
            .child(
                ContextMeter::new("inspector-context-meter", &context)
                    .label(self.text(TextKey::Context)),
            )
            .child(metric(
                self.text(TextKey::Prompt),
                &usage.prompt_tokens.to_string(),
                cx,
            ))
            .child(metric(
                self.text(TextKey::Completion),
                &usage.completion_tokens.to_string(),
                cx,
            ))
            .child(metric(
                self.text(TextKey::Cached),
                &usage.cache_hit_tokens.to_string(),
                cx,
            ))
            .child(metric(
                self.text(TextKey::Reasoning),
                &usage.reasoning_tokens.to_string(),
                cx,
            ))
            .child(metric(
                self.text(TextKey::Window),
                &usage
                    .context_window
                    .map(|window| window.to_string())
                    .unwrap_or_else(|| "—".into()),
                cx,
            ))
            .when(!cards.is_empty(), |body| {
                let mut body = body
                    .child(section_label(self.text(TextKey::RetainedEvidence), cx))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => {
                                    "Evidence the run still holds in context."
                                }
                                tact_catalog::CatalogLocale::Zh => {
                                    "本轮运行仍保留在上下文中的证据。"
                                }
                            }),
                    );
                for card in cards {
                    body = body.child(card);
                }
                body
            })
            .into_any_element()
    }

    /// Hook contexts and tool outputs the run still carries, newest first.
    fn retained_evidence(&self) -> Vec<(RowId, bool, String, String)> {
        let mut evidence = Vec::new();
        for index in (0..self.snapshot.transcript.len()).rev() {
            let Some(row) = self.snapshot.transcript.get(index) else {
                continue;
            };
            match &row.body {
                RowBody::HookContext(hook) if !hook.markdown.trim().is_empty() => {
                    evidence.push((
                        row.id,
                        true,
                        hook.source
                            .as_deref()
                            .unwrap_or(self.text(TextKey::HookSources))
                            .to_string(),
                        evidence_snippet(&hook.markdown),
                    ));
                }
                RowBody::ToolGroup(group) => {
                    for item in group.items.iter().rev() {
                        let ToolGroupItem::Tool(tool) = item else {
                            continue;
                        };
                        let Some(detail) = tool.output.detail.as_deref() else {
                            continue;
                        };
                        if detail.trim().is_empty() {
                            continue;
                        }
                        let source = if tool.arg_summary.is_empty() {
                            tool.name.to_string()
                        } else {
                            format!("{} · {}", tool.name, tool.arg_summary)
                        };
                        evidence.push((tool.id, false, source, evidence_snippet(detail)));
                    }
                }
                _ => {}
            }
            if evidence.len() >= 4 {
                break;
            }
        }
        evidence.truncate(4);
        evidence
    }

    fn hook_context_reader(&self, id: RowId) -> Option<(String, String)> {
        self.snapshot
            .transcript
            .iter()
            .find_map(|row| match &row.body {
                RowBody::HookContext(hook) if row.id == id => {
                    let source = hook
                        .source
                        .as_deref()
                        .unwrap_or(self.text(TextKey::HookSources));
                    let title = hook_context_title(&hook.markdown, source);
                    Some((
                        if title == source {
                            source.to_string()
                        } else {
                            format!("{source} · {title}")
                        },
                        hook.markdown.to_string(),
                    ))
                }
                _ => None,
            })
    }

    fn render_gate_banner(
        &self,
        gate: tact_desktop_core::GateCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .debug_selector(|| "gate-banner".into())
            .w_full()
            .min_w_0()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().warning)
            .bg(cx.theme().warning.opacity(0.08))
            .px_3()
            .py_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(gate_prompt_line(&gate.prompt).to_string()),
            )
            .child(
                div()
                    .id("review-gate")
                    .debug_selector(|| "review-gate".into())
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(self.text(TextKey::Review))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(self.text(TextKey::Review))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        let group = this.snapshot.transcript.iter().find_map(|row| {
                            let RowBody::ToolGroup(group) = &row.body else {
                                return None;
                            };
                            group
                                .items
                                .iter()
                                .any(|item| {
                                    matches!(
                                        item,
                                        ToolGroupItem::Gate(candidate)
                                            if candidate.request_id == gate.request_id
                                    )
                                })
                                .then(|| {
                                    let tool_to_expand = match &gate.origin {
                                        tact_desktop_core::GateOrigin::Tool { tool_id }
                                        | tact_desktop_core::GateOrigin::Subagent {
                                            tool_id: Some(tool_id),
                                            ..
                                        } => group.items.iter().find_map(|item| {
                                            let ToolGroupItem::Tool(tool) = item else {
                                                return None;
                                            };
                                            (tool.tool_id == *tool_id && !tool.expanded)
                                                .then_some(tool.id)
                                        }),
                                        _ => None,
                                    };
                                    (row.id, group.collapsed, tool_to_expand)
                                })
                        });
                        if let Some(index) = this.snapshot.transcript.iter().position(|row| {
                            matches!(
                                &row.body,
                                RowBody::Gate(candidate)
                                    if candidate.request_id == gate.request_id
                            ) || matches!(
                                &row.body,
                                RowBody::ToolGroup(group)
                                    if group.items.iter().any(|item| matches!(
                                        item,
                                        ToolGroupItem::Gate(candidate)
                                            if candidate.request_id
                                                == gate.request_id
                                    ))
                            )
                        }) {
                            this.scroller.update(cx, |state, cx| {
                                state.scroll_to_item(index, cx);
                            });
                            // Review focuses the primary allow option without
                            // choosing it, the way the prototype's review
                            // action does. The row owns the handle so it
                            // survives the transcript's row virtualization.
                            if let Some(row) = this.snapshot.transcript.get(index) {
                                let session_id = this.transcript_session_id();
                                if let Some(view) =
                                    this.transcript_rows.get(&(session_id, row.id)).cloned()
                                {
                                    let focus = view.read(cx).gate_focus.clone();
                                    focus.focus(window, cx);
                                }
                            }
                        }
                        if let Some((group_id, collapsed, tool_to_expand)) = group {
                            if let Some(tool_id) = tool_to_expand {
                                this.dispatch(DesktopIntent::ToggleTool {
                                    group: group_id,
                                    tool: tool_id,
                                });
                            }
                            if collapsed {
                                this.dispatch(DesktopIntent::ToggleGroup { id: group_id });
                            }
                        }
                    })),
            )
            .into_any_element()
    }

    fn render_sticky(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = crate::ui::sticky::pending_gate(&self.snapshot.pending_gates).cloned();
        let all_tasks = &self.snapshot.inspector.tasks;
        let completed = all_tasks
            .iter()
            .filter(|task| {
                matches!(
                    task.status,
                    tact_desktop_core::TaskStatusSnapshot::Completed
                )
            })
            .count();
        let task_total = all_tasks.len();
        let current = crate::ui::sticky::active_task(&self.snapshot.inspector);
        let tasks: Vec<&tact_desktop_core::TaskView> = if self.sticky_tasks_collapsed {
            current.into_iter().collect()
        } else {
            all_tasks.iter().collect()
        };
        // One header per status change, then that group's rows. The number
        // counts painted rows, so a grouped strip still reads `01 … 27` top to
        // bottom instead of jumping to the original order.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let groups = crate::ui::sticky::group_tasks(&tasks, now_ms);
        let (window, more) = crate::ui::sticky::window_rows(&groups, self.sticky_tasks_scroll);
        // A group's count covers the whole list, not the painted window:
        // scrolling must not change a header's number. `groups` is already in
        // `GROUPS` order and has at most a couple of entries, so a match on
        // status reads better than a map keyed by it.
        let count_of = |status: GroupStatus| {
            groups
                .iter()
                .find(|group| group.status == status)
                .map_or(0, |group| group.tasks.len())
        };
        let mut grouped: Vec<AnyElement> = Vec::new();
        let mut heading: Option<GroupStatus> = None;
        let mut painted = 0usize;
        for (status, task) in window {
            // The window can start mid-group, so the header is emitted on the
            // first row of each status actually painted. A group with no rows
            // in the window gets no header, and the previous heading carries
            // across the boundary either way.
            if heading != Some(status) {
                if !grouped.is_empty() {
                    grouped.push(div().h(px(2.)).w_full().into_any_element());
                }
                let count = count_of(status);
                grouped.push(
                    h_flex()
                        .id(SharedString::from(format!("sticky-task-group-{status:?}")))
                        .debug_selector({
                            let selector = format!("sticky-task-group-{status:?}");
                            move || selector.clone()
                        })
                        .role(Role::Status)
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.text(Self::sticky_status_key(status))),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_family("Lilex")
                                .text_color(cx.theme().muted_foreground)
                                .child(count.to_string()),
                        )
                        .into_any_element(),
                );
                heading = Some(status);
            }
            painted += 1;
            grouped.push(
                h_flex()
                    .id(SharedString::from(format!("sticky-task-{}", task.id)))
                    .debug_selector({
                        let selector = format!("sticky-task-row-{}", task.id);
                        move || selector.clone()
                    })
                    .role(Role::Status)
                    .aria_label(task.subject.to_string())
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(Self::marker_for(status, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(task.subject.to_string()),
                    )
                    // The row's metadata, in the kit's order: owner, what it is
                    // waiting on, then how long it has been going. Each piece
                    // is dropped when the producer never recorded it, rather
                    // than printed as a zero.
                    .child(Self::task_meta(task, now_ms, cx))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{painted:02}")),
                    )
                    .into_any_element(),
            );
        }
        // What the window cut off, reported in the row it displaced rather than
        // as an extra row — the kit's `scroll_window` does the same.
        if let Some(more) = more {
            grouped.push(
                div()
                    .id("sticky-tasks-more")
                    .debug_selector(|| "sticky-tasks-more".into())
                    .role(Role::Status)
                    .w_full()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(more)
                    .into_any_element(),
            );
        }
        let progress = if task_total == 0 {
            0.0
        } else {
            completed as f32 / task_total as f32
        };
        let goals = self.snapshot.inspector.goals.len();
        let agents = self.snapshot.inspector.subagents.len();
        let background = self.snapshot.inspector.background.len();
        // The TUI host hides its sticky when every domain is quiet; the card
        // merges those domains, so it paints while any of them would keep the
        // TUI strip up. An all-completed task list on its own is not enough.
        let card_visible = crate::ui::sticky::card_visible(
            all_tasks,
            &self.snapshot.inspector.goals,
            &self.snapshot.inspector.subagents,
            &self.snapshot.inspector.background,
        );
        let chevron = if self.sticky_tasks_collapsed {
            IconName::ChevronDown
        } else {
            IconName::ChevronUp
        };

        v_flex()
            .id("sticky-strip")
            .debug_selector(|| "sticky-strip".into())
            .role(Role::Status)
            .aria_label(self.text(TextKey::RunStatus))
            .w_full()
            .max_w(px(900.))
            .mx_auto()
            .gap_2()
            .pt_2()
            .pb(px(6.))
            .when_some(pending, |strip, gate| {
                strip.child(self.render_gate_banner(gate, cx))
            })
            .when(card_visible, |strip| {
                strip.child(
                    v_flex()
                        .id("sticky-task-card")
                        .debug_selector(|| "sticky-task-card".into())
                        .w_full()
                        .min_w_0()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .p_3()
                        .gap_2()
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} {completed}/{task_total}",
                                            self.text(TextKey::Tasks).to_uppercase()
                                        )),
                                )
                                .when_some(current, |header, task| {
                                    header.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .font_semibold()
                                            .child(task.subject.to_string()),
                                    )
                                })
                                .when(current.is_none(), |header| header.child(div().flex_1()))
                                .child(sticky_count(
                                    self.text(TextKey::Goals),
                                    goals,
                                    InspectorTab::Goals,
                                    cx,
                                ))
                                .child(sticky_count(
                                    self.text(TextKey::Subagents),
                                    agents,
                                    InspectorTab::Subagents,
                                    cx,
                                ))
                                .when(background > 0, |header| {
                                    header.child(sticky_count(
                                        self.text(TextKey::Background),
                                        background,
                                        InspectorTab::Background,
                                        cx,
                                    ))
                                })
                                .child(
                                    div()
                                        .id("toggle-sticky-tasks")
                                        .debug_selector(|| "toggle-sticky-tasks".into())
                                        .role(Role::Button)
                                        .focusable()
                                        .tab_stop(true)
                                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                        .aria_label(if self.sticky_tasks_collapsed {
                                            self.text(TextKey::ExpandStickyTasks)
                                        } else {
                                            self.text(TextKey::CollapseStickyTasks)
                                        })
                                        .cursor_pointer()
                                        .rounded_md()
                                        .p_1()
                                        .hover(|style| style.bg(cx.theme().muted))
                                        .child(
                                            Icon::new(chevron)
                                                .size(px(13.))
                                                .text_color(cx.theme().muted_foreground),
                                        )
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.sticky_tasks_collapsed =
                                                !this.sticky_tasks_collapsed;
                                            // A collapsed strip shows one row, so
                                            // a remembered offset would reopen
                                            // mid-list with no way to see the
                                            // rows above.
                                            this.sticky_tasks_scroll = 0;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .h(px(3.))
                                .w_full()
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(progress))
                                        .rounded_full()
                                        .bg(if completed == task_total {
                                            cx.theme().success
                                        } else {
                                            cx.theme().primary
                                        }),
                                ),
                        )
                        .when(!self.sticky_tasks_collapsed || task_total == 1, |card| {
                            card.child(
                                v_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_1()
                                    .children(grouped)
                                    // The strip is a summary, so its overflow
                                    // scrolls inside it rather than growing the
                                    // card over the transcript above.
                                    .on_scroll_wheel(cx.listener(
                                        |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                            let delta =
                                                f32::from(event.delta.pixel_delta(px(20.)).y);
                                            if delta == 0. {
                                                return;
                                            }
                                            let step = delta.signum() as i64;
                                            let next = (this.sticky_tasks_scroll as i64 - step)
                                                .max(0)
                                                as usize;
                                            if next != this.sticky_tasks_scroll {
                                                this.sticky_tasks_scroll = next;
                                                cx.notify();
                                            }
                                        },
                                    )),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    /// The marker a task row carries for its status.
    fn marker_for(status: GroupStatus, cx: &Context<Self>) -> AnyElement {
        match status {
            GroupStatus::Pending => div()
                .flex_none()
                .size(px(12.))
                .rounded_full()
                .border_1()
                .border_color(cx.theme().muted_foreground)
                .into_any_element(),
            GroupStatus::InProgress => Icon::new(IconName::LoaderCircle)
                .size(px(13.))
                .text_color(cx.theme().info)
                .into_any_element(),
        }
    }

    /// A task row's metadata: owner, blocking dependency, elapsed time.
    ///
    /// Mirrors `agent_tui_kit::state::task_panel`'s row: the same three facts in
    /// the same order, each omitted when unreported so the row never claims a
    /// zero it has no evidence for.
    fn task_meta(
        task: &tact_desktop_core::TaskView,
        now_ms: i64,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut parts: Vec<String> = Vec::new();
        if !task.owner.is_empty() {
            parts.push(task.owner.to_string());
        }
        if !task.blocked_by.is_empty() {
            let ids: Vec<String> = task.blocked_by.iter().map(|id| format!("#{id}")).collect();
            parts.push(format!("⏳ {}", ids.join(", ")));
        }
        if let Some(elapsed) =
            crate::ui::sticky::elapsed_label(task.started_at, task.completed_at, now_ms)
        {
            parts.push(format!("⏱ {elapsed}"));
        }
        div()
            .flex_none()
            .text_xs()
            .font_family("Lilex")
            .text_color(cx.theme().muted_foreground)
            .child(parts.join("  "))
            .into_any_element()
    }

    /// The heading the strip paints above a status group.
    fn sticky_status_key(status: GroupStatus) -> TextKey {
        match status {
            GroupStatus::InProgress => TextKey::InProgress,
            GroupStatus::Pending => TextKey::Pending,
        }
    }

    fn render_failure(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(failure) = self.snapshot.failure.clone() else {
            return div().into_any_element();
        };
        h_flex()
            .id("failure-banner")
            .role(Role::Alert)
            .aria_label(failure.message.to_string())
            .justify_between()
            .gap_3()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().danger)
            .bg(cx.theme().danger.opacity(0.08))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(failure.message.to_string()),
            )
            .child(
                div()
                    .id("failure-new-session")
                    .flex_none()
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(self.text(TextKey::NewSession))
                    .cursor_pointer()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .px_2()
                    .py_1()
                    .text_xs()
                    .child(self.text(TextKey::NewSession))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                        this.dispatch(DesktopIntent::NewSession);
                    })),
            )
            .child(
                div()
                    .id("failure-manage-sessions")
                    .flex_none()
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(self.text(TextKey::ManageSessions))
                    .cursor_pointer()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .px_2()
                    .py_1()
                    .text_xs()
                    .child(self.text(TextKey::ManageSessions))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        this.session_project_filter = this.snapshot.project.id.clone();
                        this.session_search.update(cx, |input, cx| {
                            input.focus(window, cx);
                        });
                        this.open_overlay(Overlay::SessionManager);
                    })),
            )
            .when(failure.retryable, |banner| {
                banner.child(
                    div()
                        .id("retry-failure")
                        .flex_none()
                        .role(Role::Button)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                        .aria_label(
                            failure
                                .retry_label
                                .as_deref()
                                .unwrap_or(self.text(TextKey::Retry)),
                        )
                        .cursor_pointer()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().border)
                        .px_2()
                        .py_1()
                        .text_xs()
                        .child(
                            failure
                                .retry_label
                                .as_deref()
                                .unwrap_or(self.text(TextKey::Retry))
                                .to_string(),
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                            this.dispatch(DesktopIntent::RetryProvider);
                        })),
                )
            })
            .into_any_element()
    }

    /// The composer's live reasoning-effort picker.
    ///
    /// Effort belongs beside the model it applies to, not behind the settings
    /// pane: the same model can want a different tier per task.
    fn render_effort_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let locale = self.snapshot.language;
        let selected = self.snapshot.composer.effort.as_deref().map(str::to_string);
        let choices = setting_choices(SettingId::ReasoningEffort, locale);
        let current_label = selected.as_ref().and_then(|value| {
            choices
                .iter()
                .find(|(_, choice)| choice == value)
                .map(|(label, _)| label.clone())
        });
        let label = i18n::text(locale, TextKey::Reasoning);
        let aria = match &current_label {
            Some(value) => format!("{label}: {value}"),
            None => label.to_string(),
        };
        let menu_selected = selected.clone();
        let weak = cx.weak_entity();
        let open_weak = weak.clone();
        Popover::new("effort-picker-popover")
            // The effort chip sits on the composer's bottom edge, beside the
            // permission chip; anchor its menu above like that one.
            .anchor(Anchor::BottomLeft)
            .open(self.effort_picker_open)
            .on_open_change(move |open, _, cx| {
                let _ = open_weak.update(cx, |this, cx| {
                    this.effort_picker_open = *open;
                    cx.notify();
                });
            })
            .trigger(
                gpui_base::Button::new("effort-picker-trigger")
                    .debug_selector(|| "effort-picker".into())
                    .role(Role::Button)
                    .tab_stop(true)
                    .aria_label(aria)
                    .selected(false)
                    .cursor_pointer()
                    .rounded(px(5.))
                    .px(px(4.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(5.))
                            .child(label)
                            .when_some(current_label, |row, value| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(cx.theme().foreground)
                                        .child(format!("· {value}")),
                                )
                            })
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    ),
            )
            .content(move |_, _, _| {
                let weak = weak.clone();
                let rows = choices
                    .iter()
                    .map(|(choice_label, choice_value)| {
                        let weak = weak.clone();
                        let value = choice_value.clone();
                        let option_label = choice_label.clone();
                        let is_selected = menu_selected.as_deref() == Some(choice_value.as_str());
                        gpui_base::Button::new(SharedString::from(format!(
                            "effort-choice-{choice_value}"
                        )))
                        .debug_selector({
                            let value = choice_value.clone();
                            move || format!("effort-choice-{value}")
                        })
                        .role(Role::RadioButton)
                        .tab_stop(true)
                        .aria_label(option_label.clone())
                        .selected(is_selected)
                        .cursor_pointer()
                        .w_full()
                        .rounded_md()
                        .px_2()
                        .h(px(24.))
                        .py_0()
                        .text_xs()
                        .when(is_selected, |button| button.bg(gpui::rgba(0x007aff22)))
                        .child(div().w_full().min_w_0().truncate().child(option_label))
                        .on_click(move |_, _, cx| {
                            let value = value.clone();
                            let _ = weak.update(cx, |this, cx| {
                                this.effort_picker_open = false;
                                this.dispatch(DesktopIntent::SetSetting {
                                    id: SettingId::ReasoningEffort,
                                    value: SettingValue::Choice(value),
                                });
                                cx.notify();
                            });
                        })
                    })
                    .collect::<Vec<_>>();
                v_flex()
                    .id("effort-picker-menu")
                    .debug_selector(|| "effort-picker-menu".into())
                    .role(Role::Menu)
                    .aria_label(label)
                    .w(px(168.))
                    .gap(px(2.))
                    .children(rows)
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_permission_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.snapshot.composer.permission_mode;
        let choices = [
            (
                PermissionModeView::Default,
                "default",
                self.text(TextKey::PermissionDefault),
            ),
            (
                PermissionModeView::Plan,
                "plan",
                self.text(TextKey::PermissionPlan),
            ),
            (
                PermissionModeView::Auto,
                "auto",
                self.text(TextKey::PermissionAuto),
            ),
        ];
        let current_label = choices
            .iter()
            .find_map(|(mode, _, label)| (*mode == selected).then_some(*label))
            .unwrap_or(self.text(TextKey::PermissionDefault));
        let weak = cx.weak_entity();
        let open_weak = weak.clone();
        Popover::new("permission-picker-popover")
            // The permission chip sits on the composer's bottom edge. Anchor
            // the menu above it so the third choice is not clipped by the
            // window.
            .anchor(Anchor::BottomLeft)
            .open(self.permission_picker_open)
            .on_open_change(move |open, _, cx| {
                let _ = open_weak.update(cx, |this, cx| {
                    this.permission_picker_open = *open;
                    cx.notify();
                });
            })
            .trigger(
                gpui_base::Button::new("permission-picker-trigger")
                    .debug_selector(|| "permission-picker".into())
                    .role(Role::Button)
                    .tab_stop(true)
                    .aria_label(format!(
                        "{}: {current_label}",
                        self.text(TextKey::PermissionMode)
                    ))
                    .selected(false)
                    .cursor_pointer()
                    .rounded(px(5.))
                    .px(px(4.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(6.))
                                    .rounded_full()
                                    .bg(cx.theme().warning),
                            )
                            .child(current_label)
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    ),
            )
            .content(move |_, _, _| {
                let weak = weak.clone();
                v_flex()
                    .id("permission-picker-menu")
                    .debug_selector(|| "permission-picker-menu".into())
                    .role(Role::Menu)
                    .aria_label(current_label)
                    // Three one-word choices do not need the width or the row
                    // height a dialog would: the menu hugs its labels.
                    .w(px(168.))
                    .gap(px(2.))
                    .children(choices.into_iter().map(|(mode, value, label)| {
                        let weak = weak.clone();
                        gpui_base::Button::new(SharedString::from(format!(
                            "permission-choice-{value}"
                        )))
                        .role(Role::RadioButton)
                        .tab_stop(true)
                        .aria_label(label)
                        .selected(mode == selected)
                        .cursor_pointer()
                        .w_full()
                        .rounded_md()
                        .px_2()
                        .h(px(24.))
                        .py_0()
                        .text_xs()
                        .when(mode == selected, |button| button.bg(gpui::rgba(0x007aff22)))
                        .child(div().w_full().min_w_0().truncate().child(label))
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.permission_picker_open = false;
                                this.dispatch(DesktopIntent::SetSetting {
                                    id: SettingId::PermissionMode,
                                    value: SettingValue::Choice(value.into()),
                                });
                                cx.notify();
                            });
                        })
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_slash_overlay(
        &self,
        overlay: &SlashOverlayState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = overlay
            .suggestions
            .iter()
            .enumerate()
            .map(|(index, suggestion)| {
                div()
                    .id(SharedString::from(format!(
                        "slash-suggestion-{}",
                        suggestion.insertion
                    )))
                    .role(Role::ListBoxOption)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(suggestion.title.clone())
                    .aria_selected(index == overlay.active)
                    .when(index == overlay.active, |row| row.bg(cx.theme().accent))
                    .cursor_pointer()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(div().text_sm().child(suggestion.title.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(suggestion.detail.clone()),
                            ),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(overlay) = this.slash_overlay.as_mut() {
                            overlay.active = index;
                        }
                        this.accept_slash_suggestion(false, window, cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        v_flex()
            .id("slash-overlay")
            .debug_selector(|| "slash-overlay".into())
            .role(Role::ListBox)
            .aria_label(self.text(TextKey::Commands))
            .w_full()
            .max_h(px(260.))
            .gap_0p5()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .p_1()
            .shadow_lg()
            .overflow_y_scroll()
            .occlude()
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .py_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::Commands).to_uppercase()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{}", overlay.suggestions.len())),
                    ),
            )
            .children(rows)
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_1()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Tab")
                    .child("↵")
                    .child("Esc"),
            )
            .into_any_element()
    }

    /// Controls rendered inside the composer's own footer: mentions, slash
    /// commands and the permission mode chip. Keeping them here makes the
    /// composer one bar instead of three stacked rows.
    fn render_composer_leading(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut leading = vec![
            div()
                .id("composer-attach")
                .role(Role::Button)
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                .aria_label(self.text(TextKey::Attachment))
                .cursor_pointer()
                .rounded(px(6.))
                .p(px(4.))
                .hover(|style| style.bg(cx.theme().muted))
                .child(
                    Icon::new(IconName::File)
                        .size(px(14.))
                        .text_color(cx.theme().muted_foreground),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.pick_attachments(cx);
                }))
                .into_any_element(),
            composer_insert_button(
                "composer-mention",
                "@",
                self.text(TextKey::MentionFile),
                "@",
                cx,
            )
            .into_any_element(),
            composer_insert_button(
                "composer-slash",
                "/",
                self.text(TextKey::InsertCommand),
                "/",
                cx,
            )
            .into_any_element(),
        ];
        // A microphone with no voice worker behind it is a dead control.
        if voice_button_visible(self.voice) {
            leading.push(
                composer_voice_button(self.voice, self.text(TextKey::VoiceInput), cx)
                    .into_any_element(),
            );
        }
        leading
    }

    fn render_composer_trailing(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        vec![
            self.render_effort_picker(cx),
            self.render_permission_picker(cx),
        ]
    }

    fn render_voice_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let listening = self.voice.listening;
        let label = if listening {
            self.text(TextKey::Listening)
        } else {
            self.text(TextKey::Transcribing)
        };
        h_flex()
            .id("voice-status")
            .debug_selector(|| "voice-status".into())
            .role(Role::Status)
            .aria_label(label)
            .w_full()
            .items_center()
            .gap(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().accent)
            .px(px(12.))
            .py(px(8.))
            .child(if listening {
                div()
                    .flex_none()
                    .size(px(8.))
                    .rounded_full()
                    .bg(cx.theme().danger)
                    .into_any_element()
            } else {
                Icon::new(IconName::LoaderCircle)
                    .size(px(14.))
                    .text_color(cx.theme().link)
                    .into_any_element()
            })
            .child(div().flex_1().text_size(px(12.)).child(label))
            .child(
                gpui_base::Button::new("voice-cancel")
                    .role(Role::Button)
                    .tab_stop(true)
                    .aria_label(self.text(TextKey::Cancel))
                    .px_2()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(self.text(TextKey::Cancel))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.dispatch(DesktopIntent::Voice(
                            tact_desktop_core::VoiceCommand::Cancel,
                        ));
                        this.voice.listening = false;
                        this.voice.transcribing = false;
                        cx.notify();
                    })),
            )
            .when(listening, |row| {
                row.child(
                    gpui_base::Button::new("voice-stop")
                        .role(Role::Button)
                        .tab_stop(true)
                        .aria_label(self.text(TextKey::Stop))
                        .rounded(px(5.))
                        .bg(cx.theme().primary)
                        .text_color(cx.theme().primary_foreground)
                        .px(px(10.))
                        .h(px(24.))
                        .text_size(px(11.))
                        .child(self.text(TextKey::Stop))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.dispatch(DesktopIntent::Voice(
                                tact_desktop_core::VoiceCommand::Stop,
                            ));
                            this.voice.listening = false;
                            this.voice.transcribing = true;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    /// Whether a run is active, so the transcript summary shows its clock.
    fn live_activity_visible(&self) -> bool {
        matches!(
            self.snapshot.session.phase,
            SessionPhase::Running | SessionPhase::Starting | SessionPhase::Opening
        )
    }

    /// Repaints the run header once a second while its elapsed clock is live.
    ///
    /// The transcript's process-clock timer repaints only its own entity. The
    /// header belongs to `DesktopRoot`, so without this timer it freezes after
    /// the last host event even though the run is still in flight.
    fn ensure_run_clock_timer(&mut self, cx: &mut Context<Self>) {
        if !self.live_activity_visible() {
            self.run_clock_timer = None;
            return;
        }
        if self.run_clock_timer.is_some() {
            return;
        }
        self.run_clock_timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.live_activity_visible()
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    /// The current activity label for the process summary.
    fn live_activity_label(&self) -> Option<String> {
        if !self.live_activity_visible() {
            return None;
        }
        let tool = self
            .snapshot
            .runtime
            .latest_tool_detail
            .clone()
            .filter(|tool| matches!(tool.status, RowStatus::Running | RowStatus::Pending));
        Some(match tool {
            Some(tool) => format!(
                "{} · {}",
                self.text(TextKey::Running),
                truncate_display_text(&tool.label, 60)
            ),
            None => match self.snapshot.runtime.current_phase {
                tact_desktop_core::RunPhase::Thinking => {
                    format!("{}…", self.text(TextKey::Thinking))
                }
                _ => self.text(TextKey::Running).to_string(),
            },
        })
    }

    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let labels = prompt_bar_labels(self.snapshot.language);
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_labels(labels, window, cx);
        });
        let queue = self
            .snapshot
            .composer
            .queue
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let message = QueuedMessage::new(index.to_string(), item.text.to_string());
                if index == self.queue_cursor {
                    message.note(
                        if self.snapshot.language == tact_catalog::CatalogLocale::Zh {
                            "已选择"
                        } else {
                            "Selected"
                        },
                    )
                } else {
                    message
                }
            })
            .collect::<Vec<_>>();
        let queue_len = queue.len();
        let weak = cx.weak_entity();
        let queue_view = MessageQueue::new("composer-queue")
            .items(queue)
            .editable(true)
            .labels(message_queue_labels(self.snapshot.language))
            .on_event(move |event, window, cx| match event {
                QueueEvent::Removed { id } => {
                    if let Ok(index) = id.parse::<usize>() {
                        let _ = weak.update(cx, |this, _| {
                            this.dispatch(DesktopIntent::RemoveQueued { index });
                        });
                    }
                }
                QueueEvent::SentNow { id } => {
                    if let Ok(index) = id.parse::<usize>() {
                        let _ = weak.update(cx, |this, _| {
                            this.dispatch(DesktopIntent::SendQueuedNow { index });
                        });
                    }
                }
                QueueEvent::EditRequested { id } => {
                    if let Ok(index) = id.parse::<usize>() {
                        let _ = weak.update(cx, |this, cx| {
                            if let Some(item) = this.snapshot.composer.queue.get(index).cloned() {
                                this.prompt.update(cx, |prompt, cx| {
                                    prompt.set_draft(item.text.to_string(), window, cx);
                                    prompt.focus(window, cx);
                                });
                                this.dispatch(DesktopIntent::RemoveQueued { index });
                            }
                        });
                    }
                }
                QueueEvent::Cleared => {
                    let _ = weak.update(cx, |this, _| {
                        for index in (0..this.snapshot.composer.queue.len()).rev() {
                            this.dispatch(DesktopIntent::RemoveQueued { index });
                        }
                    });
                }
                QueueEvent::MovedUp { id } | QueueEvent::MovedDown { id } => {
                    if let Ok(index) = id.parse::<usize>() {
                        let to = if matches!(event, QueueEvent::MovedUp { .. }) {
                            index.saturating_sub(1)
                        } else {
                            (index + 1).min(queue_len.saturating_sub(1))
                        };
                        let _ = weak.update(cx, |this, _| {
                            this.dispatch(DesktopIntent::MoveQueued { from: index, to });
                        });
                    }
                }
            });

        v_flex()
            .id("composer-area")
            .debug_selector(|| "composer-area".into())
            .w_full()
            .gap_2()
            .px(px(22.))
            .pb(px(18.))
            .when(self.voice.listening || self.voice.transcribing, |area| {
                area.child(self.render_voice_status(cx))
            })
            .when(!self.snapshot.composer.queue.is_empty(), |area| {
                area.child(
                    div()
                        .id("composer-queue-toggle")
                        .role(Role::Button)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                        .aria_label(format!(
                            "{} {} {queue_len}",
                            self.text(TextKey::Queue),
                            self.text(TextKey::Expand)
                        ))
                        .cursor_pointer()
                        .rounded_full()
                        .border_1()
                        .border_color(cx.theme().border)
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().link)
                        .bg(cx.theme().accent)
                        .child(format!(
                            "{} {queue_len}",
                            self.text(TextKey::Queued).to_uppercase()
                        ))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.queue_expanded = !this.queue_expanded;
                            cx.notify();
                        })),
                )
                .when(self.queue_expanded, |area| {
                    area.child(
                        div()
                            .track_focus(&self.queue_focus)
                            .key_context("Queue")
                            .w_full()
                            // The queue is a composer accessory, not a
                            // dialog: it stays a step narrower than the
                            // composer frame and never taller than a few
                            // rows. A long queue scrolls instead of
                            // pushing the transcript off screen. The cap
                            // belongs on this wrapper: on the scroll
                            // content it would clamp the content itself
                            // and nothing past the first screenful could
                            // be reached.
                            .max_w(px(820.))
                            .max_h(px(240.))
                            .mx_auto()
                            .child(div().w_full().overflow_y_scrollbar().child(queue_view)),
                    )
                })
            })
            .when_some(self.slash_overlay.clone(), |area, overlay| {
                area.key_context("SlashOverlay")
                    .child(self.render_slash_overlay(&overlay, cx))
            })
            .child(
                div()
                    .id("composer-frame")
                    .debug_selector(|| "composer-frame".into())
                    .w_full()
                    .max_w(px(900.))
                    .mx_auto()
                    .child(self.prompt.clone()),
            )
            .into_any_element()
    }

    fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if matches!(self.overlay, Some(Overlay::Reader(_))) {
            return self.render_reader_overlay(window, cx);
        }
        let overlay = self.overlay.clone()?;
        match overlay {
            Overlay::CommandCenter => {
                let compact = f32::from(window.viewport_size().height) < 420.0;
                if compact {
                    self.command_search
                        .update(cx, |search, cx| search.set_caption(None, cx));
                } else {
                    let total =
                        tact_desktop_core::catalog_entries(tact_catalog::builtins()).count();
                    let caption = match self.snapshot.language {
                        tact_catalog::CatalogLocale::En => format!(
                            "{} · {} OF {} · TYPE TO SEARCH",
                            if self.command_query.trim().is_empty() {
                                "RECENT COMMANDS"
                            } else {
                                "RESULTS"
                            },
                            self.command_hit_count,
                            total
                        ),
                        tact_catalog::CatalogLocale::Zh => format!(
                            "{} · {} / {} · 输入以搜索",
                            if self.command_query.trim().is_empty() {
                                "最近命令"
                            } else {
                                "搜索结果"
                            },
                            self.command_hit_count,
                            total
                        ),
                    };
                    self.command_search.update(cx, |search, cx| {
                        search.set_caption(Some(caption.into()), cx)
                    });
                }
                Some(
                    v_flex()
                        .id("command-center-overlay")
                        .debug_selector(|| "command-center-overlay".into())
                        .role(Role::Dialog)
                        .aria_label(self.text(TextKey::CommandCenter))
                        .w(px(
                            (f32::from(window.viewport_size().width) - 32.0).clamp(320.0, 700.0)
                        ))
                        .max_h(overlay_height(window, 620.))
                        .gap(px(10.))
                        .px(px(15.))
                        .pt(px(14.))
                        .pb(px(10.))
                        .rounded(px(13.))
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .shadow_lg()
                        .overflow_y_scroll()
                        .child(self.command_search.clone())
                        .when(!compact, |dialog| {
                            let hints = match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => {
                                    ["↑↓ move", "↵ run", "Esc close"]
                                }
                                tact_catalog::CatalogLocale::Zh => {
                                    ["↑↓ 移动", "↵ 运行", "Esc 关闭"]
                                }
                            };
                            dialog.child(
                                h_flex()
                                    .flex_none()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .children(hints),
                            )
                        })
                        .into_any_element(),
                )
            }
            Overlay::Settings => Some(self.render_settings_overlay(window, cx)),
            Overlay::Form(form) => {
                let entered = self.form_input.read(cx).value();
                let can_submit =
                    form.choice.is_none() && (!form.required || !entered.trim().is_empty());
                let input_label = form.field.clone().unwrap_or_else(|| form.title.to_string());
                let show_field = form
                    .field
                    .as_deref()
                    .is_some_and(|field| field != form.title.as_ref());
                Some(
                    v_flex()
                        .id("command-form-overlay")
                        .debug_selector(|| "command-form-overlay".into())
                        .role(Role::Dialog)
                        .aria_label(form.title.clone())
                        .w(overlay_width(window, 480.))
                        .p_4()
                        .gap_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_semibold()
                                .child(form.title.clone()),
                        )
                        .when(show_field, |panel| {
                            panel.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(form.field.clone().unwrap_or_default()),
                            )
                        })
                        .when(form.choice.is_none(), |panel| {
                            panel.child(Input::new(&self.form_input).aria_label(input_label))
                        })
                        .when(matches!(&form.action, FormAction::CreateGoal), |panel| {
                            let criteria_label = self.text(TextKey::Criteria);
                            panel
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(criteria_label),
                                )
                                .child(Input::new(&self.criteria_input).aria_label(criteria_label))
                        })
                        .when_some(form.choice.clone(), |panel, choices| {
                            panel.child(v_flex().gap_1().children(choices.into_iter().map(
                                |(label, value)| {
                                    gpui_base::Button::new(SharedString::from(format!(
                                        "form-choice-{value}"
                                    )))
                                    .debug_selector({
                                        let selector = format!("form-choice-{value}");
                                        move || selector.clone()
                                    })
                                    .aria_label(label.clone())
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .px_2()
                                    .py_1()
                                    .child(label)
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.choose_form_option(value.clone(), window, cx);
                                        },
                                    ))
                                },
                            )))
                        })
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    gpui_base::Button::new("cancel-form")
                                        .aria_label(self.text(TextKey::Cancel))
                                        .px_2()
                                        .py_1()
                                        .child(self.text(TextKey::Cancel))
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, window, cx| {
                                                this.close_overlay();
                                                this.restore_focus(window, cx);
                                                cx.notify();
                                            },
                                        )),
                                )
                                .child(
                                    gpui_base::Button::new("submit-form")
                                        .aria_label(self.text(TextKey::Confirm))
                                        .rounded_md()
                                        .bg(cx.theme().primary)
                                        .text_color(cx.theme().primary_foreground)
                                        .px_2()
                                        .py_1()
                                        .child(self.text(TextKey::Confirm))
                                        .when(!can_submit, |button| {
                                            button
                                                .bg(cx.theme().muted)
                                                .text_color(cx.theme().muted_foreground)
                                        })
                                        .when(can_submit, |button| {
                                            button.on_click(cx.listener(
                                                |this, _: &ClickEvent, window, cx| {
                                                    this.submit_form(window, cx);
                                                },
                                            ))
                                        }),
                                ),
                        )
                        .into_any_element(),
                )
            }
            // Readers are rendered before this match, because mounting their
            // typed surface (a diff table) needs `&mut self`.
            Overlay::Reader(_) => None,
            Overlay::ModelPicker(id) => Some(self.render_model_picker(id, window, cx)),
            Overlay::Subagent(child_id) => Some(self.render_subagent_overlay(child_id, window, cx)),
            Overlay::Hooks => Some(self.render_hooks_overlay(window, cx)),
            Overlay::Extensions(tab) => Some(self.render_extensions_overlay(tab, window, cx)),
            Overlay::SessionManager => Some(self.render_session_manager_overlay(window, cx)),
        }
    }

    fn render_reader_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut reader = match self.overlay.as_ref() {
            Some(Overlay::Reader(reader)) => reader.clone(),
            _ => return None,
        };
        let body = self.reader_body(&mut reader, window, cx);
        let truncated = match reader.reference.as_ref() {
            Some(ReaderRef::ToolOutput(row)) => self
                .tool_row(*row)
                .is_some_and(|tool| tool.output.truncated),
            _ => false,
        };
        if let Some(Overlay::Reader(stored)) = &mut self.overlay {
            stored.artifact_view = reader.artifact_view;
            stored.selection = reader.selection.take();
            stored.markdown_view = reader.markdown_view.take();
            stored.diff_table = reader.diff_table.take();
            stored.decisions = std::mem::take(&mut reader.decisions);
            stored.diff_epoch = reader.diff_epoch;
            stored.diff_applied = reader.diff_applied;
        }
        let title = reader.title.clone();
        let help = matches!(title.as_ref(), "Keyboard Shortcuts" | "键盘快捷键");
        let balance = reader.typed == Some(ReaderKind::Balance);
        let contents = reader.markdown.lines().count().max(1) as f32;
        let content_height = (180.0 + contents * 22.0).clamp(340.0, 640.0);
        let balance_height = (180.0 + contents * 22.0).clamp(300.0, 480.0);
        Some(
            v_flex()
                .id("reader-overlay")
                .debug_selector(|| "reader-overlay".into())
                .role(Role::Dialog)
                .aria_label(title.clone())
                .w(overlay_width(
                    window,
                    if help {
                        560.
                    } else if balance {
                        520.
                    } else {
                        760.
                    },
                ))
                .h(overlay_height(
                    window,
                    if help {
                        520.
                    } else if balance {
                        balance_height
                    } else {
                        content_height
                    },
                ))
                .p_4()
                .gap_3()
                .overflow_hidden()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .shadow_lg()
                .child(
                    h_flex()
                        .debug_selector(|| "reader-header".into())
                        .justify_between()
                        .child(div().font_semibold().child(title))
                        .child(
                            div()
                                .id("close-reader")
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(self.text(TextKey::Close))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.close_overlay();
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                }))
                                .child(self.text(TextKey::Close)),
                        ),
                )
                .when(truncated, |panel| {
                    panel.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(self.text(TextKey::ToolOutputTruncated)),
                    )
                })
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element(),
        )
    }

    /// Picks the typed surface for the row behind a reader.
    ///
    /// A tool's terminal output is painted with its ANSI colours, a web
    /// search becomes `SearchResults`, a written file becomes an
    /// `ArtifactPanel`, and a patch that touches several files becomes a
    /// `DiffTable`. Everything else stays Markdown.
    fn reader_body(
        &mut self,
        reader: &mut ReaderState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if reader.typed == Some(ReaderKind::Balance) {
            return InsightCard::untitled("balance-reader-card")
                .body(reader.markdown.to_string())
                .into_any_element();
        }
        let dark = self.snapshot.appearance == Appearance::Dark;
        match reader.reference.clone() {
            Some(ReaderRef::ToolOutput(row)) => {
                if let Some(tool) = self.tool_row(row).cloned() {
                    let detail = tool
                        .output
                        .detail
                        .as_deref()
                        .unwrap_or(reader.markdown.as_ref());
                    if web_search::is_search_tool(&tool.name) {
                        let hits = web_search::parse(detail);
                        if !hits.is_empty() {
                            let query = web_search::query(&tool.name, &tool.arg_summary);
                            return div()
                                .debug_selector(|| "reader-search".into())
                                .w_full()
                                .child(
                                    SearchResults::new("reader-search", query)
                                        .results(hits.iter().map(|hit| {
                                            SearchResult::new(hit.url.clone(), hit.title.clone())
                                                .domain(hit.domain.clone())
                                        }))
                                        .on_event(|event, _, cx| {
                                            let SearchResultsEvent::Opened { id } = event;
                                            cx.open_url(id.as_ref());
                                        }),
                                )
                                .into_any_element();
                        }
                    }
                    if let tact_desktop_core::ToolBody::Code { language, text } = &tool.body
                        && !text.trim().is_empty()
                    {
                        let kind = match language.as_deref() {
                            Some("markdown" | "md") => ArtifactKind::Markdown,
                            _ => ArtifactKind::Code,
                        };
                        let mut artifact = Artifact::new(
                            format!("reader-artifact-{:?}", tool.id),
                            if tool.arg_summary.is_empty() {
                                tool.name.to_string()
                            } else {
                                tool.arg_summary.to_string()
                            },
                            text.to_string(),
                        )
                        .kind(kind);
                        if let Some(language) = language {
                            artifact = artifact.language(language.to_string());
                        }
                        let weak = cx.weak_entity();
                        return div()
                            .h(px(420.))
                            .w_full()
                            .debug_selector(|| "reader-artifact-panel".into())
                            .child(
                                ArtifactPanel::new("reader-artifact", &artifact)
                                    .view(reader.artifact_view)
                                    .on_event(move |event, _, cx| {
                                        let ArtifactPanelEvent::ViewSelected { view, .. } = event
                                        else {
                                            return;
                                        };
                                        let _ = weak.update(cx, |this, cx| {
                                            if let Some(Overlay::Reader(reader)) = &mut this.overlay
                                            {
                                                reader.artifact_view = *view;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            )
                            .into_any_element();
                    }
                    if ansi::contains_ansi(detail) {
                        return ansi::element("reader-ansi", &ansi::parse(detail), dark);
                    }
                }
                reader_markdown_element(reader, window, cx)
            }
            Some(ReaderRef::Background(id)) if !id.is_empty() => {
                if let Some(task) = self
                    .snapshot
                    .inspector
                    .background
                    .iter()
                    .find(|task| task.id.as_ref() == id)
                    && ansi::contains_ansi(&task.output_tail)
                {
                    let elapsed = task
                        .finished_at
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
                        .saturating_sub(task.started_at)
                        .max(0) as u64;
                    let elapsed = crate::ui::background::format_duration_ms(elapsed);
                    return v_flex()
                        .debug_selector(|| "reader-background-ansi".into())
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .child(task.command.to_string()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} · {elapsed}",
                                            background_status_label(
                                                task.status,
                                                self.snapshot.language
                                            )
                                        )),
                                ),
                        )
                        .child(ansi::element(
                            "reader-ansi",
                            &ansi::parse(&task.output_tail),
                            dark,
                        ))
                        .into_any_element();
                }
                reader_markdown_element(reader, window, cx)
            }
            Some(ReaderRef::Diff(row)) => {
                if let Some(patch) = self.diff_row_patch(row).cloned()
                    && diff_review::is_multi_file(&patch)
                {
                    let files = diff_review::files(&patch);
                    return self.diff_table_body(reader, &files, window, cx);
                }
                reader_markdown_element(reader, window, cx)
            }
            _ => reader_markdown_element(reader, window, cx),
        }
    }

    fn tool_row(&self, id: RowId) -> Option<&ToolRow> {
        self.snapshot.transcript.iter().find_map(|row| {
            let RowBody::ToolGroup(group) = &row.body else {
                return None;
            };
            group.items.iter().find_map(|item| match item {
                ToolGroupItem::Tool(tool) if tool.id == id => Some(tool),
                _ => None,
            })
        })
    }

    fn diff_row_patch(&self, id: RowId) -> Option<&Arc<str>> {
        self.snapshot
            .transcript
            .iter()
            .find_map(|row| match &row.body {
                RowBody::Diff(diff) if row.id == id => Some(&diff.patch),
                _ => None,
            })
    }

    fn diff_table_body(
        &mut self,
        reader: &mut ReaderState,
        files: &[diff_review::FileDiff],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if reader.decisions.len() != files.len() {
            reader.decisions = vec![DiffProposalState::Pending; files.len()];
            reader.diff_table = None;
        }
        if reader.diff_table.is_none() {
            let label = self.text(TextKey::ChangedFiles);
            let table = cx.new(|cx| DiffTable::new("reader-diff", label, window, cx));
            table.update(cx, |table, cx| {
                table.set_labels(
                    DiffTableLabels {
                        accept: self.text(TextKey::Accept).into(),
                        reject: self.text(TextKey::Reject).into(),
                    },
                    cx,
                );
                table.set_columns(
                    [
                        DiffColumn::new("file", self.text(TextKey::File)).width_in_rems(20.),
                        DiffColumn::new("added", "+").fixed(true).width_in_rems(3.5),
                        DiffColumn::new("removed", "−")
                            .fixed(true)
                            .width_in_rems(3.5),
                    ],
                    window,
                    cx,
                );
                table.set_rows(diff_rows(files, &reader.decisions), window, cx);
                if !files.is_empty() {
                    // Review is a decision surface: start on the first file
                    // so its accept/reject actions are visible.
                    table.set_selected_row("file-0", window, cx);
                }
            });
            let subscription = cx.subscribe(&table, |this, _, event, cx| {
                if let DiffTableEvent::DecisionRequested { row_id, action, .. } = event
                    && let Some(index) = row_id
                        .as_ref()
                        .strip_prefix("file-")
                        .and_then(|index| index.parse::<usize>().ok())
                    && let Some(Overlay::Reader(reader)) = &mut this.overlay
                    && index < reader.decisions.len()
                {
                    reader.decisions[index] = match action {
                        DiffProposalAction::Accept => DiffProposalState::Accepted,
                        DiffProposalAction::Reject => DiffProposalState::Rejected,
                    };
                    reader.diff_epoch = reader.diff_epoch.wrapping_add(1);
                }
                cx.notify();
            });
            self._subscriptions.push(subscription);
            reader.diff_table = Some(table);
            reader.diff_applied = reader.diff_epoch;
        } else if let Some(table) = &reader.diff_table {
            table.update(cx, |table, cx| {
                table.set_labels(
                    DiffTableLabels {
                        accept: self.text(TextKey::Accept).into(),
                        reject: self.text(TextKey::Reject).into(),
                    },
                    cx,
                );
                table.set_columns(
                    [
                        DiffColumn::new("file", self.text(TextKey::File)).width_in_rems(20.),
                        DiffColumn::new("added", "+").fixed(true).width_in_rems(3.5),
                        DiffColumn::new("removed", "−")
                            .fixed(true)
                            .width_in_rems(3.5),
                    ],
                    window,
                    cx,
                );
            });
            if reader.diff_epoch != reader.diff_applied {
                table.update(cx, |table, cx| {
                    table.set_rows(diff_rows(files, &reader.decisions), window, cx);
                });
                reader.diff_applied = reader.diff_epoch;
            }
        }
        div()
            .h(px(260.))
            .w_full()
            .debug_selector(|| "reader-diff-table".into())
            .child(reader.diff_table.clone().unwrap())
            .into_any_element()
    }

    fn render_model_picker(
        &mut self,
        target: SettingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let models = self.model_picker_models();
        let selected = if target == SettingId::SubagentModel {
            self.snapshot.composer.subagent_model.clone()
        } else {
            self.snapshot.composer.model.clone()
        };
        let title = if target == SettingId::SubagentModel {
            self.text(TextKey::SubagentModel)
        } else {
            self.text(TextKey::Model)
        };
        let model_count = models.len();
        let model_start = self
            .model_picker_scroll
            .start(model_count, MODEL_PICKER_VISIBLE_ROWS);
        let visible_rows = model_count.min(MODEL_PICKER_VISIBLE_ROWS);
        let rows = models
            .iter()
            .enumerate()
            .skip(model_start)
            .take(MODEL_PICKER_VISIBLE_ROWS)
            .map(|(index, model)| {
                let value = model.to_string();
                let active = index == self.model_picker_cursor;
                let chosen = model.as_ref() == selected.as_ref();
                gpui_base::Button::new(SharedString::from(format!(
                    "model-choice-{target:?}-{value}"
                )))
                .role(Role::RadioButton)
                .tab_stop(true)
                .aria_label(value.clone())
                .aria_selected(chosen)
                .selected(chosen)
                .cursor_pointer()
                .w_full()
                .rounded_md()
                .px_2()
                // A fixed row height is what lets the keyboard keep its
                // cursor on screen without measuring: the visible band is
                // arithmetic.
                .h(px(MODEL_PICKER_ROW_HEIGHT))
                .py_0()
                .bg(surface::selection_fill(chosen, active, false, cx))
                .hover(|style| style.bg(surface::selection_hover_fill(chosen, false, cx)))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .size(px(14.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(chosen, |slot| {
                                    slot.child(
                                        div().size(px(5.)).rounded_full().bg(cx.theme().primary),
                                    )
                                }),
                        )
                        .child(div().min_w_0().flex_1().truncate().child(value.clone())),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.model_picker_cursor = index;
                    this.activate_model_picker(window, cx);
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let list_height = MODEL_PICKER_ROW_HEIGHT * visible_rows as f32;
        let list_scroll = self.model_picker_scroll.clone();
        let weak_for_list = cx.weak_entity();
        v_flex()
            .id("model-picker-overlay")
            .role(Role::Dialog)
            .aria_label(title)
            .w(overlay_width(window, 460.))
            .max_h(px(520.))
            .p_4()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_size(px(18.)).font_semibold().child(title))
                    .child(
                        div()
                            .id("close-model-picker")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Close))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_overlay();
                                this.restore_focus(window, cx);
                                cx.notify();
                            }))
                            .child(self.text(TextKey::Close)),
                    ),
            )
            .child(surface::quiet_search_input(&self.model_search, window, cx))
            .when(rows.is_empty(), |dialog| {
                dialog.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(if self.model_picker_query.trim().is_empty() {
                            self.text(TextKey::NoModels)
                        } else {
                            self.text(TextKey::NoMatchingModels)
                        }),
                )
            })
            .child(
                div()
                    .id("model-picker-list")
                    .relative()
                    .w_full()
                    .h(px(list_height))
                    .child(
                        div()
                            .id("model-picker-scroll")
                            .size_full()
                            .overflow_hidden()
                            .on_scroll_wheel(move |event, _, cx| {
                                let delta: f32 = event.delta.pixel_delta(px(20.)).y.into();
                                if list_scroll.wheel(
                                    delta,
                                    model_count,
                                    MODEL_PICKER_VISIBLE_ROWS,
                                    MODEL_PICKER_ROW_HEIGHT,
                                ) {
                                    let weak = weak_for_list.clone();
                                    let _ = weak.update(cx, |_, cx| cx.notify());
                                }
                                cx.stop_propagation();
                            })
                            .child(v_flex().w_full().gap_0().children(rows)),
                    ),
            )
            .into_any_element()
    }

    fn render_subagent_overlay(
        &mut self,
        child_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        let transcript = self.snapshot.transcript.iter().find_map(|row| {
            let RowBody::ToolGroup(group) = &row.body else {
                return None;
            };
            group.items.iter().find_map(|item| {
                let ToolGroupItem::Tool(tool) = item else {
                    return None;
                };
                (tool.meta.subagent_id.as_deref() == Some(child_id.as_str()))
                    .then(|| tool.subagent.clone())
                    .flatten()
            })
        });
        let session_id = self.transcript_session_id();
        let selections = transcript
            .as_ref()
            .map(|transcript| {
                self.sync_assistant_selections(transcript, &child_id, &session_id, window, cx)
            })
            .unwrap_or_default();
        let rows = transcript
            .as_ref()
            .map(|transcript| {
                transcript
                    .iter()
                    .map(|row| {
                        let selection = selections
                            .get(&(child_id.clone(), session_id.clone(), row.id))
                            .cloned();
                        transcript_row_element(
                            row,
                            &self.snapshot,
                            weak.clone(),
                            selection,
                            true,
                            None,
                            false,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let empty = transcript.is_none();
        let phase = run_phase_label(self.snapshot.runtime.current_phase, self.snapshot.language);
        v_flex()
            .id("subagent-overlay")
            .debug_selector(|| "subagent-overlay".into())
            .role(Role::Dialog)
            .aria_label(format!(
                "{} {child_id}",
                self.text(TextKey::SubagentTranscript)
            ))
            .h_full()
            .w(px(520.))
            .gap(px(0.))
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_lg()
            .child(
                v_flex()
                    .gap(px(8.))
                    .p(px(20.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .items_start()
                            .child(
                                v_flex()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .font_family("Lilex")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(match self.snapshot.language {
                                                tact_catalog::CatalogLocale::En => "SUBAGENT",
                                                tact_catalog::CatalogLocale::Zh => "子代理",
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(20.))
                                            .font_semibold()
                                            .child(child_id.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(self.text(TextKey::SubagentTranscript)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-subagent")
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(self.text(TextKey::Close))
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.text(TextKey::Close))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.close_overlay();
                                        this.restore_focus(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(
                                h_flex()
                                    .gap(px(5.))
                                    .items_center()
                                    .rounded_full()
                                    .bg(cx.theme().success.opacity(0.14))
                                    .px(px(7.))
                                    .py(px(2.))
                                    .child(div().size(px(5.)).rounded_full().bg(cx.theme().success))
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .font_family("Lilex")
                                            .text_color(cx.theme().success)
                                            .child(phase),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} {}",
                                        self.snapshot.runtime.turns_taken,
                                        match self.snapshot.language {
                                            tact_catalog::CatalogLocale::En => "turns",
                                            tact_catalog::CatalogLocale::Zh => "轮",
                                        }
                                    )),
                            ),
                    ),
            )
            .child(if empty {
                v_flex()
                    .id("subagent-transcript")
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .p(px(20.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::NoLiveTranscript)),
                    )
                    .into_any_element()
            } else {
                v_flex()
                    .id("subagent-transcript")
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .p(px(20.))
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_settings_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let settings = crate::ui::settings::rows("")
            .map(|row| {
                let setting = row.id;
                let descriptor = tact_catalog::builtins()
                    .get(&CommandId::Setting(setting))
                    .expect("every setting has a catalog descriptor");
                let label = descriptor.label.get(self.snapshot.language);
                let description = descriptor.description.get(self.snapshot.language);
                div()
                    .id(SharedString::from(format!("setting-{}", setting.as_str())))
                    .debug_selector(move || format!("setting-row-{}", setting.as_str()))
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(label)
                    .cursor_pointer()
                    .rounded_md()
                    .px_2()
                    .py(px(6.))
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .flex_1()
                                    .gap_0p5()
                                    .child(div().text_sm().child(label))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(description),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .max_w(px(140.))
                                            .truncate()
                                            .text_xs()
                                            .font_semibold()
                                            .child(setting_display_value(setting, &self.snapshot)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(setting_mode_label(
                                                setting,
                                                self.snapshot.language,
                                            )),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        if matches!(setting, SettingId::Model | SettingId::SubagentModel) {
                            this.open_model_picker(setting, cx);
                        } else {
                            this.open_setting_form(setting, window, cx);
                        }
                    }))
            })
            .collect::<Vec<_>>();

        v_flex()
            .id("settings-overlay")
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::Settings))
            .w(overlay_width(window, 680.))
            .max_h(px(620.))
            .p_4()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_semibold()
                            .child(self.text(TextKey::Settings)),
                    )
                    .child(
                        div()
                            .id("close-settings")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Close))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_overlay();
                                this.restore_focus(window, cx);
                                cx.notify();
                            }))
                            .child(self.text(TextKey::Close)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.text(TextKey::StartupSettingsHelp)),
            )
            .child(
                v_flex()
                    .id("settings-list")
                    .debug_selector(|| "settings-list".into())
                    .gap_1()
                    .overflow_y_scroll()
                    .children(settings),
            )
            .into_any_element()
    }

    fn render_hooks_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (trusted, pending) = match &self.snapshot.surface {
            Some(SurfaceSnapshot::Hooks(snapshot)) => {
                (snapshot.trusted.clone(), snapshot.pending.clone())
            }
            _ => (Vec::new(), Vec::new()),
        };
        let sources = group_hook_sources(&trusted, &pending);
        let pending_sources = sources.iter().filter(|(_, _, trusted)| !trusted).count();
        let height = (210.0 + sources.len() as f32 * 66.0).clamp(360.0, 620.0);
        let rows = sources
            .iter()
            .map(|(source, items, trusted)| {
                let detail = items
                    .iter()
                    .map(|item| match &item.matcher {
                        Some(matcher) => format!("{} · {} {}", item.event, matcher, item.command),
                        None => format!("{} · {}", item.event, item.command),
                    })
                    .collect::<Vec<_>>()
                    .join(" · ");
                let source_for_click = source.clone();
                let trusted = *trusted;
                let source_id = SharedString::from(format!("hook-source-{source}"));
                let debug_source = source_id.clone();
                h_flex()
                    .id(source_id)
                    .debug_selector(move || debug_source.to_string())
                    .justify_between()
                    .items_center()
                    .gap(px(12.))
                    .p(px(10.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(10.))
                            .items_center()
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(28.))
                                    .rounded(px(7.))
                                    .bg(cx.theme().muted)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::SquareTerminal)
                                            .size(px(14.))
                                            .text_color(cx.theme().muted_foreground),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_semibold()
                                            .truncate()
                                            .child(source.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(detail),
                                    ),
                            ),
                    )
                    .when(!trusted, |row| {
                        row.child(
                            gpui_base::Button::new(SharedString::from(format!(
                                "hook-action-{source}"
                            )))
                            .aria_label(self.text(TextKey::Trust))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(cx.theme().border)
                            .px(px(10.))
                            .py(px(4.))
                            .text_size(px(11.))
                            .child(self.text(TextKey::Trust))
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, _| {
                                    this.dispatch(DesktopIntent::InvokeCommand {
                                        id: CommandId::Cli(tact_catalog::CliEntry::HooksTrust),
                                        args: CommandArguments {
                                            tokens: vec![
                                                "--source".into(),
                                                source_for_click.clone(),
                                            ],
                                        },
                                    });
                                    // The driver answers with a fresh surface; ask
                                    // for it explicitly so the sheet cannot go stale.
                                    this.dispatch(DesktopIntent::InvokeCommand {
                                        id: CommandId::BuiltinPath {
                                            root: BuiltinCommand::Hooks,
                                            path: vec!["hooks", "list"],
                                        },
                                        args: CommandArguments::default(),
                                    });
                                },
                            )),
                        )
                    })
            })
            .collect::<Vec<_>>();

        v_flex()
            .id("hooks-overlay")
            .debug_selector(|| "hooks-overlay".into())
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::Hooks))
            .w(overlay_width(window, 780.))
            .h(px(height))
            .rounded(px(14.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .justify_between()
                    .items_start()
                    .gap(px(16.))
                    .px(px(20.))
                    .pt(px(19.))
                    .pb(px(16.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::SlashCommand)),
                    )
                    .child(
                        div()
                            .font_family("SF Pro Display")
                            .text_size(px(26.))
                            .font_semibold()
                            .child(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => "Command hooks",
                                tact_catalog::CatalogLocale::Zh => "命令钩子",
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => {
                                    "Review the exact hook sources Tact would execute. Nothing runs until its source is trusted."
                                }
                                tact_catalog::CatalogLocale::Zh => {
                                    "检查 Tact 会执行的钩子来源；未受信任前不会运行。"
                                }
                            }),
                    ),
                    )
                    .child(
                        div()
                            .id("close-hooks")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Close))
                            .cursor_pointer()
                            .text_size(px(14.))
                            .text_color(cx.theme().muted_foreground)
                            .child("×")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_overlay();
                                this.restore_focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .px(px(20.))
                    .pt(px(14.))
                    .pb(px(6.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::HookSources)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{pending_sources} {}",
                                self.text(TextKey::NeedsReview)
                            )),
                    ),
            )
            .child(
                div()
                    .mx(px(20.))
                    .flex_none()
                    // The cap sits on this wrapper, not on the scroll
                    // content, so a long hook list can scroll instead of
                    // being clamped to its first screenful.
                    .max_h(px(320.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .w_full()
                            .overflow_y_scrollbar()
                            .when(rows.is_empty(), |list| {
                                list.child(
                                    div()
                                        .p(px(16.))
                                        .text_size(px(12.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(match self.snapshot.language {
                                            tact_catalog::CatalogLocale::En => {
                                                "No hooks configured."
                                            }
                                            tact_catalog::CatalogLocale::Zh => "未配置钩子。",
                                        }),
                                )
                            })
                            .children(rows),
                    ),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .px(px(20.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => format!(
                                    "{pending_sources} sources need review"
                                ),
                                tact_catalog::CatalogLocale::Zh => {
                                    format!("{pending_sources} 个来源待审阅")
                                }
                            }),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                gpui_base::Button::new("hooks-forget-all")
                                    .aria_label(self.text(TextKey::ForgetAll))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .px(px(12.))
                                    .py(px(6.))
                                    .text_size(px(12.))
                                    .child(self.text(TextKey::ForgetAll))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                        this.dispatch(DesktopIntent::InvokeCommand {
                                            id: CommandId::Cli(
                                                tact_catalog::CliEntry::HooksForget,
                                            ),
                                            args: CommandArguments {
                                                tokens: vec!["--all".into()],
                                            },
                                        });
                                        this.refresh_hooks_surface();
                                    })),
                            )
                            .child(
                                gpui_base::Button::new("hooks-trust-all")
                                    .aria_label(self.text(TextKey::TrustAll))
                                    .rounded(px(6.))
                                    .bg(cx.theme().primary)
                                    .text_color(cx.theme().primary_foreground)
                                    .px(px(12.))
                                    .py(px(6.))
                                    .text_size(px(12.))
                                    .child(self.text(TextKey::TrustAll))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                        this.dispatch(DesktopIntent::InvokeCommand {
                                            id: CommandId::Cli(
                                                tact_catalog::CliEntry::HooksTrust,
                                            ),
                                            args: CommandArguments {
                                                tokens: vec!["--all".into()],
                                            },
                                        });
                                        this.refresh_hooks_surface();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_extensions_overlay(
        &mut self,
        tab: ExtensionsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let manage = self.extension_manage;
        let enabled_count = match tab {
            ExtensionsTab::Skills => self
                .snapshot
                .extensions
                .snapshot
                .skills
                .iter()
                .filter(|skill| skill.enabled)
                .count(),
            ExtensionsTab::Plugins => self
                .snapshot
                .extensions
                .snapshot
                .plugins
                .iter()
                .filter(|plugin| plugin.enabled)
                .count(),
            ExtensionsTab::Mcp => match &self.snapshot.surface {
                Some(SurfaceSnapshot::McpServers(servers)) => servers
                    .iter()
                    .filter(|server| {
                        matches!(
                            server.status,
                            tact_desktop_core::McpServerStatusSnapshot::Connected { .. }
                        )
                    })
                    .count(),
                _ => 0,
            },
            ExtensionsTab::Hooks => 0,
        };
        let has_table_rows = if manage {
            self.sync_extensions_table(tab, window, cx)
        } else {
            true
        };
        let query = self.extension_query.clone();
        let matches = |value: &str| crate::ui::extensions::matches_extension(&query, value);
        // Hooks has its own sheet and is not a tab in the design's
        // Extensions popover/manager.
        let tabs = [
            (ExtensionsTab::Mcp, self.text(TextKey::Mcp)),
            (ExtensionsTab::Skills, self.text(TextKey::Skills)),
            (ExtensionsTab::Plugins, self.text(TextKey::Plugins)),
        ]
        .into_iter()
        .map(|(candidate, label)| {
            div()
                .id(SharedString::from(format!("extensions-tab-{label}")))
                .role(Role::Tab)
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                .aria_label(label)
                .aria_selected(candidate == tab)
                .cursor_pointer()
                .rounded(px(0.))
                .px(px(10.))
                .pt(px(8.))
                .pb(px(7.))
                .text_size(px(12.))
                .when(candidate == tab, |button| {
                    button
                        .text_color(cx.theme().foreground)
                        .border_b_2()
                        .border_color(cx.theme().primary)
                })
                .when(candidate != tab, |button| {
                    button.text_color(cx.theme().muted_foreground)
                })
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(Overlay::Extensions(current)) = &mut this.overlay {
                        *current = candidate;
                    } else {
                        this.open_overlay(Overlay::Extensions(candidate));
                    }
                    let id = match candidate {
                        ExtensionsTab::Mcp => CommandId::Builtin(BuiltinCommand::Mcp),
                        ExtensionsTab::Skills => CommandId::Builtin(BuiltinCommand::Skill),
                        ExtensionsTab::Plugins => CommandId::Builtin(BuiltinCommand::Plugin),
                        ExtensionsTab::Hooks => CommandId::Builtin(BuiltinCommand::Hooks),
                    };
                    this.dispatch(DesktopIntent::InvokeCommand {
                        id,
                        args: CommandArguments::default(),
                    });
                    cx.notify();
                }))
        })
        .collect::<Vec<_>>();

        let mut rows = Vec::new();
        match tab {
            ExtensionsTab::Mcp => {
                if let Some(surface) = &self.snapshot.surface {
                    match surface {
                        SurfaceSnapshot::McpServers(servers) => {
                            let mut matched = servers
                                .iter()
                                .filter(|server| matches(&server.name))
                                .collect::<Vec<_>>();
                            matched.sort_by_key(|server| {
                                -crate::ui::extensions::score_extension(
                                    &query,
                                    &[&server.name, &server.transport],
                                )
                            });
                            for server in matched {
                                let name = server.name.clone();
                                let auth_name = name.clone();
                                let remove_name = name.clone();
                                let logout_name = name.clone();
                                rows.push(
                                    h_flex()
                                        .justify_between()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .p_2()
                                        .child(div().text_sm().child(name.clone()))
                                        .child(div().text_xs().child(format!(
                                            "{} · {}",
                                            mcp_status_label(
                                                &server.status,
                                                self.snapshot.language
                                            ),
                                            server.transport
                                        )))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .when(
                                                    matches!(
                                                        server.status,
                                                        tact_desktop_core::McpServerStatusSnapshot::NeedsAuthorization
                                                    ),
                                                    |actions| {
                                                        actions.child(
                                                            extension_action(
                                                                format!("mcp-auth-{name}"),
                                                                self.text(TextKey::Authorize),
                                                                DesktopIntent::InvokeCommand {
                                                                    id: CommandId::BuiltinPath {
                                                                        root: BuiltinCommand::Mcp,
                                                                        path: vec!["mcp", "auth"],
                                                                    },
                                                                    args: CommandArguments {
                                                                        tokens: vec![auth_name.clone()],
                                                                    },
                                                                },
                                                                cx,
                                                            ),
                                                        )
                                                    },
                                                )
                                                .child(extension_action(
                                                    format!("mcp-remove-{name}"),
                                                    self.text(TextKey::Remove),
                                                    DesktopIntent::InvokeCommand {
                                                        id: CommandId::Cli(
                                                            tact_catalog::CliEntry::McpRemove,
                                                        ),
                                                        args: CommandArguments {
                                                            tokens: vec![remove_name.clone()],
                                                        },
                                                    },
                                                    cx,
                                                ))
                                                .child(extension_action(
                                                    format!("mcp-remove-user-{name}"),
                                                    self.text(TextKey::RemoveUser),
                                                    DesktopIntent::InvokeCommand {
                                                        id: CommandId::Cli(
                                                            tact_catalog::CliEntry::McpRemove,
                                                        ),
                                                        args: CommandArguments {
                                                            tokens: vec![
                                                                remove_name,
                                                                "--user".into(),
                                                            ],
                                                        },
                                                    },
                                                    cx,
                                                ))
                                                .child(extension_action(
                                                    format!("mcp-logout-{name}"),
                                                    self.text(TextKey::Logout),
                                                    DesktopIntent::InvokeCommand {
                                                        id: CommandId::Cli(
                                                            tact_catalog::CliEntry::McpLogout,
                                                        ),
                                                        args: CommandArguments {
                                                            tokens: vec![logout_name],
                                                        },
                                                    },
                                                    cx,
                                                )),
                                        )
                                        .into_any_element(),
                                );
                            }
                        }
                        SurfaceSnapshot::McpPrompts(prompts) => {
                            for prompt in prompts.iter().filter(|prompt| matches(&prompt.name)) {
                                if let Some(error) = &prompt.error {
                                    rows.push(
                                        div()
                                            .id(SharedString::from(format!(
                                                "mcp-prompt-error-{}-{}",
                                                prompt.server, prompt.name
                                            )))
                                            .aria_label(error.to_string())
                                            .rounded_md()
                                            .border_1()
                                            .border_color(cx.theme().danger)
                                            .p_2()
                                            .text_xs()
                                            .child(format!("{}: {error}", prompt.server))
                                            .into_any_element(),
                                    );
                                    continue;
                                }
                                rows.push(
                                    v_flex()
                                        .gap_0p5()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .p_2()
                                        .child(
                                            div().text_sm().child(format!(
                                                "{}/{}",
                                                prompt.server, prompt.name
                                            )),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(
                                                    prompt.description.clone().unwrap_or_default(),
                                                ),
                                        )
                                        .when(!prompt.arguments.is_empty(), |row| {
                                            row.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "{}: {}",
                                                        match self.snapshot.language {
                                                            tact_catalog::CatalogLocale::En => {
                                                                "Arguments"
                                                            }
                                                            tact_catalog::CatalogLocale::Zh => {
                                                                "参数"
                                                            }
                                                        },
                                                        prompt
                                                            .arguments
                                                            .iter()
                                                            .map(|argument| {
                                                                if argument.required {
                                                                    format!("{}*", argument.name)
                                                                } else {
                                                                    argument.name.clone()
                                                                }
                                                            })
                                                            .collect::<Vec<_>>()
                                                            .join(", ")
                                                    )),
                                            )
                                        })
                                        .child(if prompt.arguments.is_empty() {
                                            extension_action(
                                                format!(
                                                    "run-mcp-prompt-{}/{}",
                                                    prompt.server, prompt.name
                                                ),
                                                match self.snapshot.language {
                                                    tact_catalog::CatalogLocale::En => "Run",
                                                    tact_catalog::CatalogLocale::Zh => "运行",
                                                },
                                                DesktopIntent::InvokeCommand {
                                                    id: CommandId::BuiltinPath {
                                                        root: BuiltinCommand::Mcp,
                                                        path: vec!["mcp", "prompt"],
                                                    },
                                                    args: CommandArguments {
                                                        tokens: vec![
                                                            prompt.server.clone(),
                                                            prompt.name.clone(),
                                                        ],
                                                    },
                                                },
                                                cx,
                                            )
                                            .into_any_element()
                                        } else {
                                            form_action_button(
                                                format!(
                                                    "run-mcp-prompt-{}/{}",
                                                    prompt.server, prompt.name
                                                ),
                                                match self.snapshot.language {
                                                    tact_catalog::CatalogLocale::En => "Run",
                                                    tact_catalog::CatalogLocale::Zh => "运行",
                                                },
                                                FormAction::McpPrompt {
                                                    server: prompt.server.clone(),
                                                    name: prompt.name.clone(),
                                                },
                                                cx,
                                            )
                                            .into_any_element()
                                        })
                                        .into_any_element(),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                let mcp_empty = !matches!(
                    &self.snapshot.surface,
                    Some(SurfaceSnapshot::McpServers(servers)) if !servers.is_empty()
                );
                if mcp_empty {
                    rows.push(
                        div()
                            .id("extensions-empty-mcp")
                            .role(Role::Status)
                            .py_3()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => "No MCP servers yet",
                                tact_catalog::CatalogLocale::Zh => "还没有 MCP 服务器",
                            })
                            .into_any_element(),
                    );
                }
                rows.push(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .id("refresh-mcp")
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(self.text(TextKey::RefreshMcp))
                                .cursor_pointer()
                                .text_xs()
                                .text_color(cx.theme().link)
                                .child(self.text(TextKey::RefreshMcp))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                    this.dispatch(DesktopIntent::InvokeCommand {
                                        id: CommandId::BuiltinPath {
                                            root: BuiltinCommand::Mcp,
                                            path: vec!["mcp", "list"],
                                        },
                                        args: CommandArguments::default(),
                                    });
                                })),
                        )
                        .into_any_element(),
                );
            }
            ExtensionsTab::Skills => {
                let mut matched = self
                    .snapshot
                    .extensions
                    .snapshot
                    .skills
                    .iter()
                    .filter(|skill| matches(&skill.name))
                    .collect::<Vec<_>>();
                matched.sort_by_key(|skill| {
                    -crate::ui::extensions::score_extension(
                        &query,
                        &[&skill.name, &skill.description],
                    )
                });
                for skill in matched {
                    let name = skill.name.clone();
                    let enabled = skill.enabled;
                    rows.push(
                        h_flex()
                            .justify_between()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .p_2()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_sm().child(skill.name.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(skill.description.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("skill-toggle-{name}")))
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(if enabled {
                                        self.text(TextKey::DisableSkill)
                                    } else {
                                        self.text(TextKey::EnableSkill)
                                    })
                                    .cursor_pointer()
                                    .text_xs()
                                    .child(if enabled {
                                        self.text(TextKey::Disable)
                                    } else {
                                        self.text(TextKey::Enable)
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                        this.dispatch(DesktopIntent::SetSkillEnabled {
                                            name: name.clone(),
                                            enabled: !enabled,
                                        });
                                    })),
                            )
                            .into_any_element(),
                    );
                }
            }
            ExtensionsTab::Plugins => {
                let mut matched = self
                    .snapshot
                    .extensions
                    .snapshot
                    .plugins
                    .iter()
                    .filter(|plugin| matches(&plugin.name))
                    .collect::<Vec<_>>();
                matched.sort_by_key(|plugin| {
                    -crate::ui::extensions::score_extension(
                        &query,
                        &[&plugin.name, &plugin.marketplace],
                    )
                });
                for plugin in matched {
                    rows.push(
                        h_flex()
                            .justify_between()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .p_2()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_sm().child(plugin.name.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{} · {}",
                                                plugin.marketplace,
                                                plugin.version.as_deref().unwrap_or(
                                                    match self.snapshot.language {
                                                        tact_catalog::CatalogLocale::En => "local",
                                                        tact_catalog::CatalogLocale::Zh => "本地",
                                                    }
                                                )
                                            )),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(extension_action(
                                        format!("plugin-update-{}", plugin.name),
                                        self.text(TextKey::Update),
                                        DesktopIntent::InvokeCommand {
                                            id: CommandId::BuiltinPath {
                                                root: BuiltinCommand::Plugin,
                                                path: vec!["plugin", "update"],
                                            },
                                            args: CommandArguments {
                                                tokens: vec![plugin.name.clone()],
                                            },
                                        },
                                        cx,
                                    ))
                                    .child(extension_action(
                                        format!("plugin-uninstall-{}", plugin.name),
                                        self.text(TextKey::Uninstall),
                                        DesktopIntent::InvokeCommand {
                                            id: CommandId::BuiltinPath {
                                                root: BuiltinCommand::Plugin,
                                                path: vec!["plugin", "uninstall"],
                                            },
                                            args: CommandArguments {
                                                tokens: vec![plugin.name.clone()],
                                            },
                                        },
                                        cx,
                                    )),
                            )
                            .into_any_element(),
                    );
                }
                rows.push(
                    extension_action(
                        "plugin-reload".into(),
                        self.text(TextKey::ReloadPlugins),
                        DesktopIntent::InvokeCommand {
                            id: CommandId::BuiltinPath {
                                root: BuiltinCommand::Plugin,
                                path: vec!["plugin", "reload"],
                            },
                            args: CommandArguments::default(),
                        },
                        cx,
                    )
                    .into_any_element(),
                );
                rows.push(
                    form_action_button(
                        "plugin-install".into(),
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => "Install plugin",
                            tact_catalog::CatalogLocale::Zh => "安装插件",
                        },
                        FormAction::Command(CommandId::Cli(tact_catalog::CliEntry::PluginInstall)),
                        cx,
                    )
                    .into_any_element(),
                );
                rows.push(
                    form_action_button(
                        "marketplace-add".into(),
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => "Add marketplace",
                            tact_catalog::CatalogLocale::Zh => "添加市场",
                        },
                        FormAction::Command(CommandId::Cli(tact_catalog::CliEntry::MarketplaceAdd)),
                        cx,
                    )
                    .into_any_element(),
                );
                for marketplace in self
                    .snapshot
                    .extensions
                    .snapshot
                    .marketplaces
                    .iter()
                    .filter(|marketplace| matches(&marketplace.name))
                {
                    let name = marketplace.name.clone();
                    rows.push(
                        h_flex()
                            .justify_between()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .p_2()
                            .text_xs()
                            .child(format!(
                                "{} · {} · {}",
                                self.text(TextKey::Marketplace),
                                marketplace.name,
                                marketplace.source
                            ))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(extension_action(
                                        format!("marketplace-update-{}", name),
                                        self.text(TextKey::Update),
                                        DesktopIntent::InvokeCommand {
                                            id: CommandId::BuiltinPath {
                                                root: BuiltinCommand::Plugin,
                                                path: vec!["plugin", "marketplace", "update"],
                                            },
                                            args: CommandArguments {
                                                tokens: vec![name.clone()],
                                            },
                                        },
                                        cx,
                                    ))
                                    .child(extension_action(
                                        format!("marketplace-remove-{}", name),
                                        self.text(TextKey::Remove),
                                        DesktopIntent::InvokeCommand {
                                            id: CommandId::BuiltinPath {
                                                root: BuiltinCommand::Plugin,
                                                path: vec!["plugin", "marketplace", "remove"],
                                            },
                                            args: CommandArguments {
                                                tokens: vec![name.clone()],
                                            },
                                        },
                                        cx,
                                    )),
                            )
                            .into_any_element(),
                    );
                }
            }
            ExtensionsTab::Hooks => {
                if let Some(SurfaceSnapshot::Hooks(hooks)) = &self.snapshot.surface {
                    let mut matched = hooks
                        .trusted
                        .iter()
                        .map(|hook| (hook, false))
                        .chain(hooks.pending.iter().map(|hook| (hook, true)))
                        .filter(|(hook, _)| matches(&hook.source))
                        .collect::<Vec<_>>();
                    matched.sort_by_key(|(hook, _)| {
                        -crate::ui::extensions::score_extension(
                            &query,
                            &[&hook.source, &hook.command],
                        )
                    });
                    for (hook, pending) in matched {
                        let source = hook.source.clone();
                        let mut row = v_flex()
                            .gap_0p5()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .p_2()
                            .child(div().text_sm().child(hook.source.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} · {}", hook.event, hook.command)),
                            );
                        if pending {
                            row = row.child(extension_action(
                                format!("hook-trust-{source}"),
                                self.text(TextKey::Authorize),
                                DesktopIntent::InvokeCommand {
                                    id: CommandId::BuiltinPath {
                                        root: BuiltinCommand::Hooks,
                                        path: vec!["hooks", "trust", "--source"],
                                    },
                                    args: CommandArguments {
                                        tokens: vec![source],
                                    },
                                },
                                cx,
                            ));
                        }
                        rows.push(row.into_any_element());
                    }
                }
                rows.push(
                    h_flex()
                        .gap_2()
                        .child(extension_action(
                            "hooks-list".into(),
                            self.text(TextKey::ListHooks),
                            DesktopIntent::InvokeCommand {
                                id: CommandId::BuiltinPath {
                                    root: BuiltinCommand::Hooks,
                                    path: vec!["hooks", "list"],
                                },
                                args: CommandArguments::default(),
                            },
                            cx,
                        ))
                        .child(extension_action(
                            "hooks-trust-all".into(),
                            self.text(TextKey::TrustAll),
                            DesktopIntent::InvokeCommand {
                                id: CommandId::BuiltinPath {
                                    root: BuiltinCommand::Hooks,
                                    path: vec!["hooks", "trust", "--all"],
                                },
                                args: CommandArguments::default(),
                            },
                            cx,
                        ))
                        .child(extension_action(
                            "hooks-forget-all".into(),
                            self.text(TextKey::ForgetAll),
                            DesktopIntent::InvokeCommand {
                                id: CommandId::BuiltinPath {
                                    root: BuiltinCommand::Hooks,
                                    path: vec!["hooks", "forget", "--all"],
                                },
                                args: CommandArguments::default(),
                            },
                            cx,
                        ))
                        .into_any_element(),
                );
            }
        }

        let visible_rows = if manage {
            self.extensions_table.read(cx).row_count()
        } else {
            0
        };
        let manager_height = (210.0 + visible_rows as f32 * 40.0).clamp(360.0, 620.0);
        v_flex()
            .id("extensions-overlay")
            .debug_selector(|| "extensions-overlay".into())
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::Extensions))
            .w(overlay_width(window, if manage { 780. } else { 520. }))
            .when(manage, |dialog| dialog.h(px(manager_height)))
            .when(!manage, |dialog| dialog.max_h(px(560.)))
            .p(if manage { px(20.) } else { px(15.) })
            .gap(if manage { px(14.) } else { px(10.) })
            .rounded(if manage { px(14.) } else { px(13.) })
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(match (self.snapshot.language, manage) {
                                        (tact_catalog::CatalogLocale::En, true) => {
                                            "EXTENSIONS MANAGER"
                                        }
                                        (tact_catalog::CatalogLocale::Zh, true) => "扩展管理",
                                        (tact_catalog::CatalogLocale::En, false) => "EXTENSIONS",
                                        (tact_catalog::CatalogLocale::Zh, false) => "扩展",
                                    }),
                            )
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .when(manage, |title| title.text_size(px(26.)))
                                    .when(!manage, |title| title.text_size(px(22.)))
                                    .font_semibold()
                                    .child(if manage {
                                        self.text(TextKey::ManageExtensions)
                                    } else {
                                        match tab {
                                            ExtensionsTab::Mcp => self.text(TextKey::Mcp),
                                            ExtensionsTab::Skills => self.text(TextKey::Skills),
                                            ExtensionsTab::Plugins => self.text(TextKey::Plugins),
                                            ExtensionsTab::Hooks => self.text(TextKey::Hooks),
                                        }
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("close-extensions")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Close))
                            .cursor_pointer()
                            .rounded(px(6.))
                            .p(px(4.))
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_overlay();
                                this.restore_focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .when(manage, |row| {
                        row.w_full().border_b_1().border_color(cx.theme().border)
                    })
                    .children(tabs),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(10.))
                    .when(self.extension_manage, |row| {
                        row.child(div().w(px(360.)).child(surface::quiet_search_input(
                            &self.extension_search,
                            window,
                            cx,
                        )))
                        .child(div().flex_1())
                    })
                    .when(!self.extension_manage, |row| {
                        row.child(div().flex_1().child(surface::quiet_search_input(
                            &self.extension_search,
                            window,
                            cx,
                        )))
                    }),
            )
            .child(if self.extension_manage {
                if has_table_rows {
                    div()
                        .id("extensions-table-host")
                        .min_h(px(240.))
                        .flex_1()
                        .child(self.extensions_table.clone())
                        .into_any_element()
                } else {
                    div()
                        .id("extensions-empty")
                        .role(Role::Status)
                        .min_h(px(120.))
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(match (tab, self.snapshot.language) {
                            (ExtensionsTab::Mcp, tact_catalog::CatalogLocale::En) => {
                                "No MCP servers yet"
                            }
                            (ExtensionsTab::Mcp, tact_catalog::CatalogLocale::Zh) => {
                                "还没有 MCP 服务器"
                            }
                            (_, tact_catalog::CatalogLocale::En) => self.text(TextKey::NoItems),
                            (_, tact_catalog::CatalogLocale::Zh) => self.text(TextKey::NoItems),
                        })
                        .into_any_element()
                }
            } else if rows.is_empty() {
                div()
                    .id("extensions-empty")
                    .role(Role::Status)
                    .min_h(px(96.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.text(TextKey::NoItems))
                    .into_any_element()
            } else {
                v_flex()
                    .id("extensions-list")
                    .gap_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(if manage {
                                if tab == ExtensionsTab::Skills {
                                    match self.snapshot.language {
                                        tact_catalog::CatalogLocale::En => {
                                            "Enter inserts · Space toggles"
                                        }
                                        tact_catalog::CatalogLocale::Zh => {
                                            "Enter 插入 · Space 开关"
                                        }
                                    }
                                    .to_string()
                                } else {
                                    self.text(TextKey::ChangesApply).to_string()
                                }
                            } else {
                                format!(
                                    "{enabled_count} {}",
                                    self.text(TextKey::Enabled).to_lowercase()
                                )
                            }),
                    )
                    .when(manage, |row| {
                        row.child(
                            gpui_base::Button::new("extensions-done")
                                .rounded(px(6.))
                                .bg(cx.theme().primary)
                                .text_color(cx.theme().primary_foreground)
                                .px(px(12.))
                                .py(px(5.))
                                .text_size(px(12.))
                                .child(self.text(TextKey::Done))
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.close_overlay();
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!manage, |row| {
                        row.child(
                            div()
                                .id("manage-extensions-footer")
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(self.text(TextKey::ManageExtensions))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(cx.theme().link)
                                .child(self.text(TextKey::Manage))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    let mut view = this.snapshot.extensions.clone();
                                    view.query = this.extension_query.clone();
                                    let dialog = crate::ui::extensions::manage(&view);
                                    this.extension_query = dialog.query;
                                    this.extension_manage = true;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_session_manager_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rename_label = self.text(TextKey::Rename);
        let title_label = argument_label("title", self.snapshot.language);
        let archive_label = self.text(TextKey::Archive);
        let restore_label = self.text(TextKey::Restore);
        // While the controller is starting a session, another switch has
        // nothing to act on: it would close a host that is still coming up.
        // The rows keep their position but stop accepting the click, so a
        // burst of clicks cannot queue a burst of starts. Teardown is
        // excluded on purpose — closing is retryable, and a click during it
        // is how a close that is taking too long is retried.
        let switching = matches!(
            self.snapshot.session.phase,
            SessionPhase::Opening | SessionPhase::Starting | SessionPhase::Switching
        );
        let manager_view = crate::ui::session_manager::SessionManagerView {
            project_filter: self
                .session_project_filter
                .as_ref()
                .map(|project| project.0.clone()),
            filter: self.snapshot.sessions.filter,
            query: self.snapshot.sessions.query.clone(),
        };
        let filtered = crate::ui::session_manager::filter_sessions(
            &self.snapshot.sessions.items,
            &manager_view,
        );
        let visible_count = filtered.len();
        let mut project_counts: HashMap<ProjectId, usize> = HashMap::new();
        for session in &filtered {
            *project_counts
                .entry(session.project_id.clone())
                .or_default() += 1;
        }
        let query_active = !self.snapshot.sessions.query.trim().is_empty();
        let mut previous_project = None;
        let rows = filtered
            .into_iter()
            .flat_map(|session| {
                let mut output = Vec::new();
                let collapsed = !query_active
                    && self
                        .collapsed_session_projects
                        .contains(&session.project_id);
                if previous_project.as_ref() != Some(&session.project_id) {
                    let project_name = self
                        .snapshot
                        .projects
                        .iter()
                        .find(|project| project.id.as_ref() == Some(&session.project_id))
                        .map(project_identity_label)
                        .or_else(|| {
                            (self.snapshot.project.id.as_ref() == Some(&session.project_id))
                                .then(|| project_identity_label(&self.snapshot.project))
                        })
                        .or_else(|| {
                            session
                                .root_dir
                                .file_name()
                                .and_then(|name| name.to_str())
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| "—".into());
                    let count = project_counts
                        .get(&session.project_id)
                        .copied()
                        .unwrap_or_default();
                    let group_label = match self.snapshot.language {
                        tact_catalog::CatalogLocale::En => {
                            format!("{project_name}, {count} sessions")
                        }
                        tact_catalog::CatalogLocale::Zh => {
                            format!("{project_name}，{count} 个会话")
                        }
                    };
                    let initials = project_initials(&project_name);
                    let toggle_project = session.project_id.clone();
                    let toggle_key_project = toggle_project.clone();
                    output.push(
                        h_flex()
                            .id(SharedString::from(format!(
                                "manager-project-group-{}",
                                session.project_id.0
                            )))
                            .debug_selector({
                                let id = session.project_id.0.clone();
                                move || format!("manager-project-group-{id}")
                            })
                            .role(Role::Button)
                            .aria_label(group_label)
                            .aria_expanded(!collapsed)
                            .focusable()
                            .tab_stop(true)
                            .w_full()
                            .items_center()
                            .gap(px(7.))
                            .rounded(px(6.))
                            .px(px(12.))
                            .pt(px(12.))
                            .pb(px(4.))
                            .cursor_pointer()
                            .hover(|style| style.bg(cx.theme().muted))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.toggle_session_project(toggle_project.clone());
                                cx.notify();
                            }))
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key == "enter" || event.keystroke.key == "space"
                                {
                                    this.toggle_session_project(toggle_key_project.clone());
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .child(
                                div()
                                    .debug_selector({
                                        let id = session.project_id.0.clone();
                                        move || format!("manager-project-group-chevron-{id}")
                                    })
                                    .flex_none()
                                    .child(
                                        Icon::new(if collapsed {
                                            IconName::ChevronRight
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .size(px(12.))
                                        .text_color(cx.theme().muted_foreground),
                                    ),
                            )
                            .child(
                                div()
                                    .debug_selector({
                                        let id = session.project_id.0.clone();
                                        move || format!("manager-project-group-leading-{id}")
                                    })
                                    .flex_none()
                                    .size(px(20.))
                                    .rounded(px(5.))
                                    .bg(cx.theme().secondary)
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(8.))
                                    .font_semibold()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(initials),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(11.))
                                    .font_semibold()
                                    .child(project_name),
                            )
                            .child(
                                div()
                                    .debug_selector({
                                        let id = session.project_id.0.clone();
                                        move || format!("manager-project-group-count-{id}")
                                    })
                                    .flex_none()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(9.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(count.to_string()),
                            )
                            .into_any_element(),
                    );
                    previous_project = Some(session.project_id.clone());
                }
                if collapsed {
                    return output;
                }
                let session_id = SessionId(session.id.clone());
                let rename_id = session_id.clone();
                let archive_id = session_id.clone();
                let archived = session.archived_at.is_some();
                let rename_value = self.session_title(session);
                let locked =
                    session.locked && self.snapshot.session.id.as_ref() != Some(&session_id);
                let current = self.snapshot.session.id.as_ref() == Some(&session_id);
                let resume_id = session_id.clone();
                let short_id = session.id.rsplit('-').next().unwrap_or(&session.id);
                let short_id = &short_id[..short_id.len().min(4)];
                let branch = self
                    .snapshot
                    .projects
                    .iter()
                    .find(|project| project.id.as_ref() == Some(&session.project_id))
                    .and_then(|project| project.branch.as_deref())
                    .or_else(|| {
                        (self.snapshot.project.id.as_ref() == Some(&session.project_id))
                            .then_some(self.snapshot.project.branch.as_deref())
                            .flatten()
                    })
                    .unwrap_or("—");
                let subtitle = format!(
                    "{} · {} {}",
                    branch,
                    self.text(TextKey::SessionShort),
                    short_id
                );
                let when = relative_session_time(session.updated_at, self.snapshot.language);
                let messages = match self.snapshot.language {
                    tact_catalog::CatalogLocale::Zh => format!("{} 条", session.message_count),
                    _ => format!("{} msgs", session.message_count),
                };
                let dot = if locked {
                    cx.theme().warning
                } else if archived {
                    cx.theme().muted_foreground
                } else {
                    cx.theme().success
                };
                let title = rename_value.clone();
                let row_label = if locked {
                    format!("{title} · {}", self.text(TextKey::Locked))
                } else {
                    title.clone()
                };
                let rename_button = gpui_base::Button::new(SharedString::from(format!(
                    "rename-session-{}",
                    session.id
                )))
                .role(Role::Button)
                .tab_stop(true)
                .aria_label(rename_label)
                .rounded(px(5.))
                .p(px(3.))
                .hover(|style| style.bg(cx.theme().muted))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child("✎"),
                )
                .on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        this.form_input.update(cx, |input, cx| {
                            input.set_value(rename_value.clone(), window, cx);
                            input.focus(window, cx);
                        });
                        this.open_overlay(Overlay::Form(FormState {
                            title: rename_label.into(),
                            action: FormAction::RenameSession(rename_id.clone()),
                            field: Some(title_label.into()),
                            required: true,
                            choice: None,
                        }));
                        cx.notify();
                    },
                ));
                let archive_button = gpui_base::Button::new(SharedString::from(format!(
                    "archive-session-{}",
                    session.id
                )))
                .debug_selector({
                    let selector = format!("archive-session-{}", session.id);
                    move || selector.clone()
                })
                .role(Role::Button)
                .tab_stop(true)
                .aria_label(if archived {
                    restore_label
                } else {
                    archive_label
                })
                .rounded(px(5.))
                .p(px(3.))
                .hover(|style| style.bg(cx.theme().muted))
                .child(
                    Icon::new(IconName::Inbox)
                        .size(px(13.))
                        .text_color(cx.theme().muted_foreground),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                    this.dispatch(DesktopIntent::SetSessionArchived {
                        session_id: archive_id.clone(),
                        archived: !archived,
                    });
                }));
                output.push(
                    div()
                        .w_full()
                        .pl(px(14.))
                        .child(
                            div()
                                .w_full()
                                .pl(px(10.))
                                .border_l(px(1.))
                                .border_color(cx.theme().border)
                                .child(
                                    h_flex()
                                        .id(SharedString::from(format!(
                                            "manager-session-{}",
                                            session.id
                                        )))
                                        .debug_selector({
                                            let selector =
                                                format!("manager-session-{}", session.id);
                                            move || selector.clone()
                                        })
                                        .w_full()
                                        .min_w_0()
                                        .items_center()
                                        .gap(px(10.))
                                        .h(px(44.))
                                        .px(px(12.))
                                        .border_b_1()
                                        .border_color(cx.theme().border)
                                        .when(current, |row| row.bg(cx.theme().accent))
                                        .child(
                                            div().flex_none().size(px(6.)).rounded_full().bg(dot),
                                        )
                                        .child(
                                            v_flex()
                                                .id(SharedString::from(format!(
                                                    "resume-session-{}",
                                                    session.id
                                                )))
                                                .role(Role::Button)
                                                .focusable()
                                                .tab_stop(!locked)
                                                .aria_label(row_label.clone())
                                                .flex_1()
                                                .min_w_0()
                                                .gap(px(1.))
                                                .when(!locked && !switching, |area| {
                                                    area.cursor_pointer().on_click(cx.listener({
                                                        let resume_id = resume_id.clone();
                                                        move |this, _: &ClickEvent, window, cx| {
                                                            this.dispatch(
                                                                DesktopIntent::ResumeSession {
                                                                    session_id: resume_id.clone(),
                                                                },
                                                            );
                                                            this.close_overlay();
                                                            this.restore_focus(window, cx);
                                                            cx.notify();
                                                        }
                                                    }))
                                                })
                                                .child(
                                                    h_flex()
                                                        .items_center()
                                                        .gap(px(6.))
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .truncate()
                                                                .text_size(px(13.))
                                                                .font_semibold()
                                                                .child(title.clone()),
                                                        )
                                                        .when(archived, |row| {
                                                            row.child(
                                                                div()
                                                                    .flex_none()
                                                                    .rounded_full()
                                                                    .bg(cx.theme().muted)
                                                                    .px(px(6.))
                                                                    .py(px(1.))
                                                                    .text_size(px(9.))
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .child(
                                                                        self.text(
                                                                            TextKey::Archived,
                                                                        ),
                                                                    ),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(10.))
                                                        .font_family("Lilex")
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(subtitle.clone()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(px(72.))
                                                .text_right()
                                                .text_size(px(10.))
                                                .font_family("Lilex")
                                                .text_color(cx.theme().muted_foreground)
                                                .child(when.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(px(56.))
                                                .text_right()
                                                .text_size(px(10.))
                                                .font_family("Lilex")
                                                .text_color(cx.theme().muted_foreground)
                                                .child(messages.clone()),
                                        )
                                        .child(rename_button)
                                        .child(archive_button),
                                ),
                        )
                        .into_any_element(),
                );
                output
            })
            .collect::<Vec<_>>();
        let all_projects_label = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => "All projects",
            tact_catalog::CatalogLocale::Zh => "全部项目",
        };
        let all_projects_selected = self.session_project_filter.is_none();
        let mut project_rows: Vec<AnyElement> = vec![
            gpui_base::Button::new("manager-project-all")
                .selected(all_projects_selected)
                .aria_label(all_projects_label)
                .w_full()
                .justify_start()
                .rounded(px(8.))
                .bg(if all_projects_selected {
                    cx.theme().accent
                } else {
                    cx.theme().transparent
                })
                .px(px(8.))
                .py(px(6.))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(px(9.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(29.))
                                .rounded(px(8.))
                                .bg(cx.theme().muted)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Folder)
                                        .size(px(14.))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .font_semibold()
                                        .child(all_projects_label),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.text(TextKey::AllSessions)),
                                ),
                        ),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.session_project_filter = None;
                    cx.notify();
                }))
                .into_any_element(),
        ];
        project_rows.extend(self.snapshot.projects.iter().map(|project| {
            let selected = project.id == self.session_project_filter;
            let project_id = project.id.clone();
            let remove_project_id = project_id.clone().expect("registered project has an id");
            let project_key = project
                .id
                .as_ref()
                .map(|id| id.0.clone())
                .unwrap_or_else(|| "none".into());
            let count = project.id.as_ref().filter(|_| project.available).map(|id| {
                crate::ui::session_manager::project_session_count(id, &self.snapshot.sessions.items)
            });
            let project_label = project_identity_label(project);
            let initials = project_initials(&project_label);
            let detail = match (self.snapshot.language, count) {
                (tact_catalog::CatalogLocale::Zh, Some(count)) => {
                    format!("{count} 个会话")
                }
                (tact_catalog::CatalogLocale::Zh, None) => "未加载会话".to_string(),
                (tact_catalog::CatalogLocale::En, Some(count)) => {
                    format!("{count} sessions")
                }
                (tact_catalog::CatalogLocale::En, None) => "No sessions loaded".to_string(),
            };
            let project_button = gpui_base::Button::new(SharedString::from(format!(
                "manager-project-{}",
                project_key
            )))
            .debug_selector({
                let project_key = project_key.clone();
                move || format!("manager-project-{project_key}")
            })
            .selected(selected)
            .aria_label(project_label.clone())
            .flex_1()
            .min_w_0()
            .justify_start()
            .rounded(px(8.))
            .bg(if selected {
                cx.theme().accent
            } else {
                cx.theme().transparent
            })
            .px(px(8.))
            .py(px(6.))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(9.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(29.))
                            .rounded(px(8.))
                            .bg(cx.theme().secondary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.))
                            .font_semibold()
                            .child(initials),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_semibold()
                                    .child(project_label.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(detail),
                            ),
                    )
                    .when_some(count, |row, count| {
                        row.child(
                            div()
                                .flex_none()
                                .font_family("Lilex")
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .child(count.to_string()),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.session_project_filter = project_id.clone();
                cx.notify();
            }));
            h_flex()
                .w_full()
                .items_center()
                .gap(px(2.))
                .child(project_button)
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "manager-remove-project-{project_key}"
                        )))
                        .debug_selector({
                            let project_key = project_key.clone();
                            move || format!("manager-remove-project-{project_key}")
                        })
                        .role(Role::Button)
                        .focusable()
                        .tab_stop(true)
                        .aria_label(self.text(TextKey::RemoveProject))
                        .cursor_pointer()
                        .size(px(28.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(cx.theme().muted))
                        .child(
                            Icon::new(IconName::Close)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.open_remove_project(remove_project_id.clone(), window, cx);
                        })),
                )
                .into_any_element()
        }));
        project_rows.push(
            gpui_base::Button::new("manager-add-project")
                .aria_label(self.text(TextKey::AddProject))
                .w_full()
                .justify_start()
                .rounded(px(8.))
                .px(px(8.))
                .py(px(6.))
                .hover(|style| style.bg(cx.theme().muted))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(px(9.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(29.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(cx.theme().border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Plus)
                                        .size(px(14.))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_semibold()
                                        .child(self.text(TextKey::AddProject)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.text(TextKey::OpenAnotherWorkspace)),
                                ),
                        ),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_overlay();
                    this.restore_focus(window, cx);
                    this.open_project_picker(cx);
                }))
                .into_any_element(),
        );

        let height = (f32::from(window.viewport_size().height) - 100.0).clamp(320.0, 720.0);
        v_flex()
            .id("session-manager-overlay")
            .debug_selector(|| "session-manager-overlay".into())
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::SessionManager))
            .w(overlay_width(window, 1040.))
            .h(px(height))
            .rounded(px(14.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_start()
                    .px(px(19.))
                    .pt(px(17.))
                    .pb(px(14.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(match self.snapshot.language {
                                        tact_catalog::CatalogLocale::En => "SESSION MANAGER",
                                        tact_catalog::CatalogLocale::Zh => "会话管理",
                                    }),
                            )
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .text_size(px(24.))
                                    .font_semibold()
                                    .child(match self.snapshot.language {
                                        tact_catalog::CatalogLocale::En => "Sessions by project",
                                        tact_catalog::CatalogLocale::Zh => "按项目查看会话",
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("close-session-manager")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Close))
                            .cursor_pointer()
                            .rounded(px(6.))
                            .p(px(4.))
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_overlay();
                                this.restore_focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .w(px(232.))
                            .h_full()
                            .min_h_0()
                            .gap(px(4.))
                            .p(px(10.))
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .px(px(8.))
                                    .py(px(6.))
                                    .text_size(px(10.))
                                    .font_family("Lilex")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.text(TextKey::Projects).to_uppercase()),
                            )
                            .children(project_rows),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .gap(px(0.))
                            .p(px(18.))
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(div().flex_1().min_w_0().child(
                                        surface::quiet_search_input(
                                            &self.session_search,
                                            window,
                                            cx,
                                        ),
                                    ))
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .gap(px(2.))
                                            .rounded(px(7.))
                                            .border_1()
                                            .border_color(cx.theme().border)
                                            .p(px(2.))
                                            .child(filter_button(
                                                self.text(TextKey::Active),
                                                self.snapshot.sessions.filter
                                                    == ArchiveFilter::Active,
                                                ArchiveFilter::Active,
                                                cx,
                                            ))
                                            .child(filter_button(
                                                self.text(TextKey::Archived),
                                                self.snapshot.sessions.filter
                                                    == ArchiveFilter::Archived,
                                                ArchiveFilter::Archived,
                                                cx,
                                            ))
                                            .child(filter_button(
                                                self.text(TextKey::All),
                                                self.snapshot.sessions.filter == ArchiveFilter::All,
                                                ArchiveFilter::All,
                                                cx,
                                            )),
                                    ),
                            )
                            .child({
                                let group_label = if self.session_project_filter.is_none() {
                                    all_projects_label.to_string()
                                } else {
                                    project_identity_label(&self.snapshot.project)
                                };
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .items_center()
                                    .px(px(2.))
                                    .pt(px(12.))
                                    .pb(px(6.))
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        div().text_size(px(12.)).font_semibold().child(group_label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .font_family("Lilex")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{visible_count} {}",
                                                self.text(TextKey::Sessions).to_lowercase()
                                            )),
                                    )
                            })
                            .when_some(self.snapshot.sessions.error.clone(), |manager, failure| {
                                manager.child(
                                    v_flex()
                                        .id("session-manager-error")
                                        .role(Role::Alert)
                                        .aria_label(failure.message.to_string())
                                        .gap_1()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(cx.theme().danger)
                                        .p_2()
                                        .text_xs()
                                        .child(failure.message.to_string())
                                        .child(
                                            div()
                                                .id("session-manager-retry")
                                                .role(Role::Button)
                                                .focusable()
                                                .tab_stop(true)
                                                .focus_visible(|style| {
                                                    style.bg(gpui::rgba(0x007aff22))
                                                })
                                                .aria_label(self.text(TextKey::Retry))
                                                .cursor_pointer()
                                                .text_color(cx.theme().link)
                                                .child(self.text(TextKey::Retry))
                                                .on_click(cx.listener(
                                                    |this, _: &ClickEvent, _, _| {
                                                        this.dispatch(
                                                            DesktopIntent::RefreshSessions,
                                                        );
                                                    },
                                                )),
                                        ),
                                )
                            })
                            .when(self.snapshot.sessions.loading, |manager| {
                                manager.child(LoadingState::new().label(
                                    match self.snapshot.language {
                                        tact_catalog::CatalogLocale::En => "Loading sessions",
                                        tact_catalog::CatalogLocale::Zh => "正在加载会话",
                                    },
                                ))
                            })
                            .child(
                                v_flex()
                                    .id("session-manager-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .children(rows),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(14.))
                    .px(px(19.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .font_family("Lilex")
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .children(
                        match self.snapshot.language {
                            tact_catalog::CatalogLocale::En => ["↑↓ move", "↵ open", "esc close"],
                            tact_catalog::CatalogLocale::Zh => ["↑↓ 移动", "↵ 打开", "esc 关闭"],
                        }
                        .into_iter()
                        .map(|hint| div().child(hint)),
                    ),
            )
            .into_any_element()
    }

    fn on_command_center(
        &mut self,
        _: &keymap::CommandCenter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_focus(window, cx);
        self.open_command_center(window, cx);
    }
}

impl Render for DesktopRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_run_clock_timer(cx);
        if self.focus_restore_pending {
            self.focus_restore_pending = false;
            self.restore_focus(window, cx);
        }
        if let Some(text) = self.pending_voice_text.take() {
            let draft = self.prompt.read(cx).draft(cx).to_string();
            let next = if draft.trim().is_empty() {
                text
            } else {
                format!("{} {text}", draft.trim_end())
            };
            self.prompt.update(cx, |prompt, cx| {
                prompt.set_draft(next, window, cx);
                prompt.focus(window, cx);
            });
            let draft = self.prompt.read(cx).draft(cx).to_string();
            self.refresh_slash_overlay(&draft, cx);
        }
        let shell_layout = crate::ui::layout::layout(
            f32::from(window.viewport_size().width),
            self.snapshot.text_scale,
            self.left_visible,
            self.right_visible,
        );
        window.set_rem_size(px(16.0 * self.snapshot.text_scale));
        if self.snapshot.language != self.last_language {
            self.last_language = self.snapshot.language;
            self.pending_row_updates
                .extend(self.snapshot.transcript.iter().map(|row| row.id));
            let sessions_placeholder = i18n::text(self.snapshot.language, TextKey::SearchSessions);
            let extensions_placeholder =
                i18n::text(self.snapshot.language, TextKey::SearchExtensions);
            let models_placeholder = i18n::text(self.snapshot.language, TextKey::SearchModels);
            let rail_placeholder = rail_search_placeholder(self.snapshot.language);
            self.rail_search.update(cx, |input, cx| {
                input.set_placeholder(rail_placeholder, window, cx);
            });
            self.session_search.update(cx, |input, cx| {
                input.set_placeholder(sessions_placeholder, window, cx);
            });
            self.extension_search.update(cx, |input, cx| {
                input.set_placeholder(extensions_placeholder, window, cx);
            });
            self.model_search.update(cx, |input, cx| {
                input.set_placeholder(models_placeholder, window, cx);
            });
        }
        if self.overlay_focus_pending && self.overlay.is_some() {
            self.overlay_focus_pending = false;
            if matches!(self.overlay, Some(Overlay::Extensions(_))) {
                self.extension_search
                    .update(cx, |input, cx| input.focus(window, cx));
            } else if matches!(self.overlay, Some(Overlay::ModelPicker(_))) {
                // The filter is the picker's keyboard entry point: typing
                // narrows the list, the arrow keys move the cursor, and Enter
                // takes the highlighted model.
                self.model_search.update(cx, |input, cx| {
                    input.set_value(String::new(), window, cx);
                    input.focus(window, cx);
                });
            } else if matches!(self.overlay, Some(Overlay::SessionManager)) {
                if self.reset_session_search_on_open {
                    self.session_search.update(cx, |input, cx| {
                        input.set_value(String::new(), window, cx);
                    });
                    self.reset_session_search_on_open = false;
                }
                self.session_search
                    .update(cx, |input, cx| input.focus(window, cx));
            } else {
                self.overlay_focus.focus(window, cx);
            }
        }
        if self.focus_composer_on_ready && self.snapshot.session.phase == SessionPhase::Ready {
            self.focus_composer_on_ready = false;
            self.prompt
                .update(cx, |prompt, cx| prompt.focus(window, cx));
        }
        let drawer_overlay = matches!(self.overlay, Some(Overlay::Subagent(_)));
        let overlay = self.render_overlay(window, cx);
        let top_biased_overlay = matches!(
            self.overlay,
            Some(Overlay::CommandCenter | Overlay::Form(_) | Overlay::ModelPicker(_))
        ) && f32::from(window.viewport_size().height) >= 500.;
        let popover_overlay =
            matches!(self.overlay, Some(Overlay::Extensions(_))) && !self.extension_manage;
        // Overlays arrive with one restrained fade and rise rather than
        // appearing between two frames. Reduced motion resolves to the settled
        // frame, as every other transition here does.
        let overlay_epoch = self.overlay_epoch;
        let overlay_direct_motion = crate::ui::shell::motion_policy(self.snapshot.motion)
            .direct_state_changes
            || cx.reduce_motion();
        let scale = self.snapshot.text_scale;
        let rail_width = px(self.rail_width * scale);
        let inspector_width = px(self.inspector_width * scale);
        let mut left_panel = self.left_visible.then(|| {
            self.render_session_rail(rail_width, window, cx)
                .into_any_element()
        });
        let inline_left = if shell_layout.rail_inline {
            left_panel.take()
        } else {
            None
        };
        let scrim_left = (!shell_layout.rail_inline).then_some(left_panel).flatten();
        let mut right_panel = self.right_visible.then(|| {
            self.render_inspector(inspector_width, cx)
                .into_any_element()
        });
        let inline_right = if shell_layout.inspector_inline {
            right_panel.take()
        } else {
            None
        };
        let scrim_right = (!shell_layout.inspector_inline)
            .then_some(right_panel)
            .flatten();

        v_flex()
            .id("tact-desktop-root")
            .debug_selector(|| "tact-desktop-root".into())
            .role(Role::Application)
            .aria_label(self.text(TextKey::Application))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_command_center))
            .on_action(cx.listener(|_, _: &keymap::Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &keymap::OpenSettings, window, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.remember_focus(window, cx);
                this.open_overlay(Overlay::Settings);
            }))
            .on_action(cx.listener(|this, _: &keymap::NewSession, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.dispatch(DesktopIntent::NewSession);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleLeft, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.left_visible = !this.left_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleRight, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.right_visible = !this.right_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::BackToBottom, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_end(cx));
                this.dispatch(DesktopIntent::BackToBottom);
            }))
            .on_action(cx.listener(|this, _: &keymap::CancelRun, window, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.request_cancel_run(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::NextInspector, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(next_inspector(this.snapshot.inspector.tab), cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::PreviousInspector, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(previous_inspector(this.snapshot.inspector.tab), cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowRun, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(InspectorTab::Run, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowGoals, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(InspectorTab::Goals, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowSubagents, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(InspectorTab::Subagents, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowContext, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(InspectorTab::Context, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowBackground, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.select_inspector(InspectorTab::Background, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::QueuePrevious, window, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.queue_cursor = crate::ui::shell::alt_up_down_keeps_queue_selection(
                    this.queue_cursor,
                    this.snapshot.composer.queue.len(),
                    -1,
                );
                this.queue_expanded = true;
                this.queue_focus.focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::QueueNext, window, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.queue_cursor = crate::ui::shell::alt_up_down_keeps_queue_selection(
                    this.queue_cursor,
                    this.snapshot.composer.queue.len(),
                    1,
                );
                this.queue_expanded = true;
                this.queue_focus.focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::QueueSendNow, _, cx| {
                if this.overlay.is_some() {
                    return;
                }
                if this.queue_cursor < this.snapshot.composer.queue.len() {
                    this.dispatch(DesktopIntent::SendQueuedNow {
                        index: this.queue_cursor,
                    });
                }
                this.queue_expanded = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::SubmitComposer, window, cx| {
                if this.overlay.is_some() {
                    return;
                }
                this.submit_composer_draft(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::SlashNext, _, cx| {
                this.move_slash_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::SlashPrevious, _, cx| {
                this.move_slash_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::SlashComplete, window, cx| {
                this.accept_slash_suggestion(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::SlashExecute, window, cx| {
                this.accept_slash_suggestion(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::SlashDismiss, _, cx| {
                this.dismiss_slash_overlay(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::Dismiss, window, cx| {
                this.dismiss_topmost(window, cx);
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some(drag) = this.divider_drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.divider_drag = None;
                    cx.notify();
                    return;
                }
                let scale = this.snapshot.text_scale;
                let delta = (f32::from(event.position.x) - drag.pointer_x) / scale;
                match drag.side {
                    DividerSide::Rail => {
                        this.rail_width = (drag.width + delta).clamp(
                            crate::ui::layout::RAIL_MIN_WIDTH,
                            crate::ui::layout::RAIL_MAX_WIDTH,
                        );
                    }
                    DividerSide::Inspector => {
                        this.inspector_width = (drag.width - delta).clamp(
                            crate::ui::layout::INSPECTOR_MIN_WIDTH,
                            crate::ui::layout::INSPECTOR_MAX_WIDTH,
                        );
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.divider_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(self.render_title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        h_flex()
                            .size_full()
                            .when_some(inline_left, |row, left| row.child(left))
                            .when(shell_layout.rail_inline, |row| {
                                row.child(self.render_divider(DividerSide::Rail, cx))
                            })
                            .child(
                                v_flex()
                                    .id("run-room")
                                    .debug_selector(|| "run-room".into())
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .child(self.render_run_tabs(cx))
                                    .child(
                                        h_flex().flex_1().min_h_0().child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .h_full()
                                                .child(div().flex_1().min_h_0().relative().child(
                                                    match self.run_tab {
                                                        RunTab::Chat => {
                                                            self.render_transcript(window, cx)
                                                        }
                                                        RunTab::Trajectory => {
                                                            self.render_trajectory_page(window, cx)
                                                        }
                                                    },
                                                ))
                                                .when(
                                                    self.snapshot.failure.is_some()
                                                        && self.run_tab == RunTab::Chat,
                                                    |column| column.child(self.render_failure(cx)),
                                                )
                                                // The ledger is evidence, not a place to
                                                // continue the conversation: the sticky
                                                // task panel and composer belong to the
                                                // Chat page only.
                                                .when(self.run_tab == RunTab::Chat, |column| {
                                                    column
                                                        .child(self.render_sticky(cx))
                                                        .child(self.render_composer(window, cx))
                                                }),
                                        ),
                                    ),
                            )
                            .when(shell_layout.inspector_inline, |row| {
                                row.child(self.render_divider(DividerSide::Inspector, cx))
                            })
                            .when_some(inline_right, |row, right| row.child(right)),
                    )
                    .when_some(scrim_right, |row, right| {
                        row.child(
                            v_flex()
                                .id("inspector-backdrop")
                                .absolute()
                                .inset_0()
                                .items_end()
                                .bg(theme::scrim_color())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.right_visible = false;
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                }))
                                .child(
                                    v_flex()
                                        .id("inspector-scrim")
                                        .w(inspector_width)
                                        .h_full()
                                        .border_l_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().background)
                                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .child(right),
                                ),
                        )
                    })
                    .when_some(scrim_left, |row, left| {
                        row.child(
                            v_flex()
                                .id("rail-backdrop")
                                .absolute()
                                .inset_0()
                                .bg(theme::scrim_color())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.left_visible = false;
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                }))
                                .child(
                                    v_flex()
                                        .id("rail-scrim")
                                        .w(rail_width)
                                        .h_full()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().background)
                                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .child(left),
                                ),
                        )
                    }),
            )
            .when_some(self.toast.clone(), |root, toast| {
                root.child(
                    h_flex()
                        .id("desktop-toast")
                        .debug_selector(|| "desktop-toast".into())
                        .role(Role::Status)
                        .aria_label(toast.title.clone())
                        .justify_between()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(div().text_sm().font_semibold().child(toast.title))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(toast.body),
                                ),
                        )
                        .child(
                            div()
                                .id("dismiss-toast")
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(self.text(TextKey::DismissNotification))
                                .cursor_pointer()
                                .child("×")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.toast = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(overlay, |root, overlay| {
                let content = div().child(
                    v_flex()
                        .id("overlay-content")
                        .when(drawer_overlay, |content| content.h_full())
                        .focus_trap("overlay-focus-trap", &self.overlay_focus)
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(overlay),
                );
                let content = if overlay_direct_motion {
                    content.into_any_element()
                } else {
                    content
                        .with_animation(
                            ("overlay-content", overlay_epoch),
                            Animation::new(Duration::from_millis(160)),
                            |content, delta| {
                                content
                                    .relative()
                                    .top(px((1.0 - delta) * 10.0))
                                    .opacity(delta)
                            },
                        )
                        .into_any_element()
                };
                root.child(
                    v_flex()
                        .id("overlay-scrim")
                        .key_context("Overlay")
                        .absolute()
                        .inset_0()
                        .when(drawer_overlay, |scrim| scrim.items_end().justify_end())
                        .when(popover_overlay, |scrim| {
                            scrim.items_center().justify_center()
                        })
                        .when(
                            !drawer_overlay && !popover_overlay && top_biased_overlay,
                            |scrim| {
                                scrim
                                    .items_center()
                                    .justify_start()
                                    .pt(px((f32::from(window.viewport_size().height) * 0.13)
                                        .clamp(60., 140.)))
                            },
                        )
                        .when(
                            !drawer_overlay && !popover_overlay && !top_biased_overlay,
                            |scrim| scrim.items_center().justify_center(),
                        )
                        .bg(if popover_overlay {
                            cx.theme().transparent
                        } else {
                            theme::scrim_color()
                        })
                        // The overlay owns the pointer and wheel surface. Without
                        // occluding the hitboxes behind it, GPUI still routes a
                        // wheel event to the transcript scroller underneath even
                        // though clicks are handled by the dialog.
                        .occlude()
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.close_overlay();
                            this.restore_focus(window, cx);
                            cx.notify();
                        }))
                        .child(content),
                )
            })
    }
}

#[derive(Clone)]
enum FormValue {
    Args(CommandArguments),
    Setting(SettingId, SettingValue),
}

fn filter_button(
    label: &'static str,
    selected: bool,
    filter: ArchiveFilter,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("filter-{label}")))
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(label)
        .aria_selected(selected)
        .cursor_pointer()
        .rounded(px(5.))
        .h(px(24.))
        .px(px(8.))
        .text_size(px(11.))
        .text_color(if selected {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .when(selected, |button| {
            button.bg(cx.theme().popover).font_semibold()
        })
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
            this.dispatch(DesktopIntent::SetSessionFilter(filter));
        }))
}

fn truncate_table_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Truncates a mono label by display columns and reserves the ellipsis width.
fn truncate_display_text(text: &str, max_columns: usize) -> String {
    if text.width() <= max_columns {
        return text.to_string();
    }
    if max_columns == 0 {
        return String::new();
    }
    let budget = max_columns.saturating_sub('…'.width().unwrap_or(1));
    let mut columns = 0;
    let mut truncated = String::new();
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if columns + width > budget {
            return format!("{truncated}…");
        }
        columns += width;
        truncated.push(ch);
    }
    truncated
}

fn relative_session_time(
    updated_at: chrono::DateTime<chrono::Utc>,
    locale: tact_catalog::CatalogLocale,
) -> String {
    let delta = chrono::Utc::now().signed_duration_since(updated_at);
    let minutes = delta.num_minutes();
    match locale {
        tact_catalog::CatalogLocale::Zh => {
            if minutes < 1 {
                "刚刚".into()
            } else if minutes < 60 {
                format!("{minutes} 分钟前")
            } else if delta.num_hours() < 24 {
                format!("{} 小时前", delta.num_hours())
            } else if delta.num_days() == 1 {
                "昨天".into()
            } else if delta.num_days() < 7 {
                format!("{} 天前", delta.num_days())
            } else {
                updated_at.format("%m月%d日").to_string()
            }
        }
        tact_catalog::CatalogLocale::En => {
            if minutes < 1 {
                "now".into()
            } else if minutes < 60 {
                format!("{minutes}m ago")
            } else if delta.num_hours() < 24 {
                format!("{}h ago", delta.num_hours())
            } else if delta.num_days() == 1 {
                "Yesterday".into()
            } else if delta.num_days() < 7 {
                format!("{}d ago", delta.num_days())
            } else {
                updated_at.format("%b %d").to_string()
            }
        }
    }
}

fn is_cjk_title(locale: tact_catalog::CatalogLocale, title: &str) -> bool {
    locale == tact_catalog::CatalogLocale::Zh
        && title
            .chars()
            .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
}

fn setting_display_value(id: SettingId, snapshot: &DesktopSnapshot) -> String {
    match id {
        SettingId::Model => snapshot
            .runtime
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
            .unwrap_or(&snapshot.composer.model)
            .to_string(),
        SettingId::SubagentModel => snapshot.composer.subagent_model.to_string(),
        SettingId::ReasoningEffort => snapshot
            .composer
            .effort
            .as_deref()
            .unwrap_or("—")
            .to_string(),
        SettingId::PermissionMode => match snapshot.composer.permission_mode {
            PermissionModeView::Default => "default".into(),
            PermissionModeView::Plan => "plan".into(),
            PermissionModeView::Auto => "auto".into(),
        },
        SettingId::Notifications => if snapshot.notifications { "on" } else { "off" }.into(),
        SettingId::HookOutput => if snapshot.show_hook_output {
            "on"
        } else {
            "off"
        }
        .into(),
        SettingId::Theme => match snapshot.appearance {
            Appearance::Light => "light".into(),
            Appearance::Dark => "dark".into(),
        },
        SettingId::ThemeFamily => snapshot.theme.label().into(),
        SettingId::Language => match snapshot.language {
            tact_catalog::CatalogLocale::En => "English".into(),
            tact_catalog::CatalogLocale::Zh => "中文".into(),
        },
        SettingId::Provider => non_empty(&snapshot.settings.provider),
        SettingId::ApiKey => {
            if snapshot.settings.api_key_set {
                "••••••".into()
            } else {
                "—".into()
            }
        }
        SettingId::BaseUrl => non_empty(&snapshot.settings.base_url),
        SettingId::MaxTokens => optional_number(snapshot.settings.max_tokens.map(u64::from)),
        SettingId::ModelContextWindow => {
            optional_number(snapshot.settings.context_window.map(|value| value as u64))
        }
        SettingId::SnapshotMaxItems => optional_number(
            snapshot
                .settings
                .snapshot_max_items
                .map(|value| value as u64),
        ),
        SettingId::MicroCompact => if snapshot.settings.micro_compact {
            "on"
        } else {
            "off"
        }
        .into(),
        SettingId::TokioConsole => {
            if snapshot.settings.tokio_console {
                "on".into()
            } else {
                "off".into()
            }
        }
        SettingId::SkillBodyAutoInject => {
            if snapshot.settings.skill_body_auto_inject {
                "on".into()
            } else {
                "off".into()
            }
        }
        SettingId::ConfigPath => snapshot
            .settings
            .config_path
            .clone()
            .unwrap_or_else(|| "—".into()),
        SettingId::ThinkingBudget => snapshot
            .composer
            .thinking_budget
            .map(|budget| budget.to_string())
            .unwrap_or_else(|| "—".into()),
    }
}

fn non_empty(value: &str) -> String {
    if value.is_empty() {
        "—".into()
    } else {
        value.to_string()
    }
}

fn optional_number(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".into())
}

fn project_initials(project: &str) -> String {
    let segments = project
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let letters = segments
        .iter()
        .rev()
        .take(2)
        .filter_map(|segment| segment.chars().find(|ch| ch.is_alphanumeric()))
        .collect::<String>();
    let letters = if letters.is_empty() {
        "T".to_string()
    } else {
        letters.chars().rev().collect()
    };
    letters.to_uppercase()
}

fn project_identity_label(project: &tact_desktop_core::ProjectView) -> String {
    project
        .repository
        .as_ref()
        .map(|repository| repository.to_string())
        .unwrap_or_else(|| project.display_name.to_string())
}

fn action_button(
    id: String,
    label: &'static str,
    intent: DesktopIntent,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(label)
        .cursor_pointer()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .px_2()
        .py_0p5()
        .text_xs()
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
            this.dispatch(intent.clone());
        }))
}

fn form_action_button(
    id: String,
    label: &'static str,
    action: FormAction,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(label)
        .cursor_pointer()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .px_2()
        .py_0p5()
        .text_xs()
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.remember_focus(window, cx);
            this.form_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            this.open_overlay(Overlay::Form(FormState {
                title: label.into(),
                action: action.clone(),
                field: Some(label.into()),
                required: true,
                choice: None,
            }));
            cx.notify();
        }))
}

fn extension_action(
    id: String,
    label: &'static str,
    intent: DesktopIntent,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(label)
        .cursor_pointer()
        .text_xs()
        .text_color(cx.theme().link)
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
            this.dispatch(intent.clone());
        }))
}

fn goal_invoke(verb: &'static str, goal_id: &str, text: Option<&str>) -> DesktopIntent {
    let mut tokens = vec![goal_id.to_string()];
    if let Some(text) = text {
        tokens.push(text.to_string());
    }
    DesktopIntent::InvokeCommand {
        id: CommandId::BuiltinPath {
            root: BuiltinCommand::Goal,
            path: vec!["goal", verb],
        },
        args: CommandArguments { tokens },
    }
}

#[cfg(test)]
fn sticky_shortcut(
    label: &'static str,
    tab: InspectorTab,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("sticky-{label}")))
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(label)
        .cursor_pointer()
        .text_xs()
        .text_color(cx.theme().link)
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.select_inspector(tab, cx);
            cx.notify();
        }))
}

fn sticky_count(
    label: &'static str,
    count: usize,
    tab: InspectorTab,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    let text = format!("{label} {count}");
    div()
        .id(SharedString::from(format!("sticky-count-{label}")))
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(text.clone())
        .cursor_pointer()
        .rounded_md()
        .px_1p5()
        .py_0p5()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .hover(|style| style.bg(cx.theme().muted))
        .child(text)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.select_inspector(tab, cx);
            cx.notify();
        }))
}

fn signal_row(
    label: &str,
    value: String,
    tone: gpui::Hsla,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .min_w_0()
        .justify_between()
        .gap_2()
        .text_size(px(11.))
        .child(
            h_flex()
                .min_w_0()
                .items_center()
                .gap(px(7.))
                .child(div().flex_none().size(px(6.)).rounded_full().bg(tone))
                .child(
                    div()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(label.to_string()),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(value),
        )
}

fn metric(label: &'static str, value: &str, cx: &mut Context<DesktopRoot>) -> impl IntoElement {
    h_flex()
        .id(SharedString::from(format!("metric-{label}")))
        .role(Role::Status)
        .aria_label(format!("{label}: {value}"))
        .w_full()
        .min_w_0()
        .gap_2()
        .justify_between()
        .text_sm()
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_right()
                .child(value.to_string()),
        )
}

fn prompt_bar_labels(locale: tact_catalog::CatalogLocale) -> PromptBarLabels {
    match locale {
        tact_catalog::CatalogLocale::En => PromptBarLabels {
            placeholder: "Message the agent; use @ mentions or / commands".into(),
            composer: "Prompt composer".into(),
            model_prefix: "Model: ".into(),
            no_models: "No models available".into(),
            available_models: "Available models".into(),
            search_models: "Search models".into(),
            no_matching_models: "No matching models".into(),
            running: "Running; cancel is available".into(),
            ready: "Ready for another prompt".into(),
            failed_prefix: "Failed: ".into(),
            attachments: "Prompt attachments".into(),
            suggestions: "Prompt suggestions".into(),
            attach: "Attach".into(),
            enhance: "Enhance".into(),
            cancel: "Cancel".into(),
            send: "Send".into(),
        },
        tact_catalog::CatalogLocale::Zh => PromptBarLabels {
            placeholder: "向代理发送消息；使用 @ 引用文件或 / 插入命令".into(),
            composer: "提示词编辑器".into(),
            model_prefix: "模型：".into(),
            no_models: "尚未配置模型".into(),
            available_models: "可用模型".into(),
            search_models: "搜索模型".into(),
            no_matching_models: "没有匹配的模型".into(),
            running: "运行中；可取消".into(),
            ready: "可以继续输入".into(),
            failed_prefix: "失败：".into(),
            attachments: "提示词附件".into(),
            suggestions: "提示词建议".into(),
            attach: "附加".into(),
            enhance: "增强".into(),
            cancel: "取消".into(),
            send: "发送".into(),
        },
    }
}

fn tool_count_label(count: usize, locale: tact_catalog::CatalogLocale) -> String {
    match locale {
        tact_catalog::CatalogLocale::En if count == 1 => "1 tool".into(),
        tact_catalog::CatalogLocale::En => format!("{count} tools"),
        tact_catalog::CatalogLocale::Zh => format!("{count} 个工具"),
    }
}

fn tool_group_title(
    group: &tact_desktop_core::ToolGroupRow,
    locale: tact_catalog::CatalogLocale,
) -> String {
    let tools = group
        .items
        .iter()
        .filter_map(|item| match item {
            ToolGroupItem::Tool(tool) => Some(tool),
            ToolGroupItem::Gate(_) => None,
        })
        .collect::<Vec<_>>();
    let mut parts = vec![tool_count_label(tools.len(), locale)];
    if group.loop_id.value() != u64::MAX {
        parts.push(match locale {
            tact_catalog::CatalogLocale::En => format!("Loop {}", group.loop_id.value() + 1),
            tact_catalog::CatalogLocale::Zh => format!("循环 {}", group.loop_id.value() + 1),
        });
    }
    if !tools.is_empty() {
        let summary = tools
            .iter()
            .take(2)
            .map(|tool| {
                if tool.arg_summary.is_empty() {
                    tool.name.to_string()
                } else {
                    // The header is one compact row: a summary that kept its
                    // own line breaks would grow the card to the command's
                    // shape. Join it as a label; the row ellipsizes the rest.
                    format!(
                        "{} {}",
                        tool.name,
                        tact_desktop_core::one_line_summary(&tool.arg_summary)
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(" · ");
        parts.push(if tools.len() > 2 {
            format!("{summary} +{}", tools.len() - 2)
        } else {
            summary
        });
    }
    let duration_ms = tools
        .iter()
        .filter_map(|tool| tool.duration_ms)
        .sum::<u64>();
    if duration_ms > 0 {
        parts.push(if duration_ms >= 1_000 {
            format!("{:.1}s", duration_ms as f64 / 1_000.0)
        } else {
            format!("{duration_ms} ms")
        });
    }
    parts.join(" · ")
}

fn argument_label(name: &str, locale: tact_catalog::CatalogLocale) -> &'static str {
    match (name, locale) {
        ("objective", tact_catalog::CatalogLocale::Zh) => "目标",
        ("objective", tact_catalog::CatalogLocale::En) => "Objective",
        ("title", tact_catalog::CatalogLocale::Zh) => "标题",
        ("title", tact_catalog::CatalogLocale::En) => "Title",
        ("value", tact_catalog::CatalogLocale::Zh) => "值",
        ("server", tact_catalog::CatalogLocale::Zh) => "服务",
        ("source", tact_catalog::CatalogLocale::Zh) => "来源",
        ("plugin", tact_catalog::CatalogLocale::Zh) => "插件",
        ("marketplace", tact_catalog::CatalogLocale::Zh) => "市场",
        ("prompt", tact_catalog::CatalogLocale::Zh) => "提示词",
        ("name", tact_catalog::CatalogLocale::Zh) => "名称",
        ("command", tact_catalog::CatalogLocale::Zh) => "命令",
        ("url", tact_catalog::CatalogLocale::Zh) => "URL",
        ("text", tact_catalog::CatalogLocale::Zh) => "文本",
        ("server name key=value...", tact_catalog::CatalogLocale::Zh) => {
            "服务器 提示词名 [键=值...]"
        }
        (_, tact_catalog::CatalogLocale::Zh) => "参数",
        ("value", tact_catalog::CatalogLocale::En) => "Value",
        ("source", tact_catalog::CatalogLocale::En) => "Source",
        ("plugin", tact_catalog::CatalogLocale::En) => "Plugin",
        ("marketplace", tact_catalog::CatalogLocale::En) => "Marketplace",
        ("server", tact_catalog::CatalogLocale::En) => "Server",
        ("prompt", tact_catalog::CatalogLocale::En) => "Prompt",
        ("name", tact_catalog::CatalogLocale::En) => "Name",
        ("command", tact_catalog::CatalogLocale::En) => "Command",
        ("url", tact_catalog::CatalogLocale::En) => "URL",
        ("text", tact_catalog::CatalogLocale::En) => "Text",
        ("server name key=value...", tact_catalog::CatalogLocale::En) => {
            "Server, prompt name, key=value..."
        }
        (_, tact_catalog::CatalogLocale::En) => "Parameter",
    }
}

fn command_search_labels(locale: tact_catalog::CatalogLocale) -> CommandSearchLabels {
    match locale {
        tact_catalog::CatalogLocale::En => CommandSearchLabels {
            container: "Command search".into(),
            placeholder: "Search commands".into(),
            empty: "No commands available".into(),
            no_results: "No matching commands".into(),
        },
        tact_catalog::CatalogLocale::Zh => CommandSearchLabels {
            container: "命令搜索".into(),
            placeholder: "搜索命令".into(),
            empty: "暂无可用命令".into(),
            no_results: "没有匹配的命令".into(),
        },
    }
}

fn message_queue_labels(locale: tact_catalog::CatalogLocale) -> MessageQueueLabels {
    match locale {
        tact_catalog::CatalogLocale::En => MessageQueueLabels::default(),
        tact_catalog::CatalogLocale::Zh => MessageQueueLabels {
            clear: "清空队列".into(),
            queued: "排队提示".into(),
            waiting: "条等待".into(),
            queued_item: "排队".into(),
            of: "/".into(),
            move_earlier: "上移".into(),
            move_later: "下移".into(),
            edit: "编辑".into(),
            send_now: "立即发送".into(),
            remove: "移除".into(),
        },
    }
}

fn plan_card_labels(locale: tact_catalog::CatalogLocale) -> PlanCardLabels {
    match locale {
        tact_catalog::CatalogLocale::En => PlanCardLabels::default(),
        tact_catalog::CatalogLocale::Zh => PlanCardLabels {
            eyebrow: "计划".into(),
            steps: "步骤".into(),
            aria_prefix: "计划".into(),
            aria_of: "/".into(),
            aria_steps_done: "项完成".into(),
            step_prefix: "步骤".into(),
            approve: "批准计划".into(),
            reject: "拒绝".into(),
            edit: "编辑计划".into(),
            proposed: "待决定".into(),
            approved: "已批准".into(),
            rejected: "已拒绝".into(),
            running: "进行中".into(),
            completed: "已完成".into(),
            failed: "失败".into(),
            pending: "待处理".into(),
            done: "完成".into(),
            skipped: "已跳过".into(),
        },
    }
}

fn sidebar_nav_labels(locale: tact_catalog::CatalogLocale) -> SidebarNavLabels {
    match locale {
        tact_catalog::CatalogLocale::En => SidebarNavLabels {
            workspace: "Workspace navigation".into(),
            filter: "Search sessions by title or id".into(),
            tree: "Session navigation items".into(),
            new_task: "New run".into(),
            collapse: "Collapse sessions".into(),
            expand: "Expand sessions".into(),
            no_items: "No sessions".into(),
            no_matches: "No matching sessions".into(),
            badge_prefix: "Badge".into(),
            contains_selected: "Contains selected item".into(),
            unavailable: "Unavailable".into(),
        },
        tact_catalog::CatalogLocale::Zh => SidebarNavLabels {
            workspace: "工作区导航".into(),
            filter: "按标题或 ID 搜索会话".into(),
            tree: "会话导航项".into(),
            new_task: "新建运行".into(),
            collapse: "收起会话栏".into(),
            expand: "展开会话栏".into(),
            no_items: "暂无会话".into(),
            no_matches: "没有匹配的会话".into(),
            badge_prefix: "标记".into(),
            contains_selected: "包含当前选中项".into(),
            unavailable: "不可用".into(),
        },
    }
}

fn thinking_labels(locale: tact_catalog::CatalogLocale) -> ThinkingLabels {
    match locale {
        tact_catalog::CatalogLocale::En => ThinkingLabels::default(),
        tact_catalog::CatalogLocale::Zh => ThinkingLabels {
            thinking: "正在思考…".into(),
            stopped: "思考已停止".into(),
            thought_for_prefix: "思考了 ".into(),
            seconds_suffix: " 秒".into(),
            thoughts: "思考".into(),
            step_running: "进行中".into(),
            step_done: "完成".into(),
        },
    }
}

fn tool_call_labels(locale: tact_catalog::CatalogLocale) -> ToolCallLabels {
    match locale {
        tact_catalog::CatalogLocale::En => ToolCallLabels::default(),
        tact_catalog::CatalogLocale::Zh => ToolCallLabels {
            input: "输入".into(),
            output: "输出".into(),
            approval: "审批".into(),
            waiting: "此调用正在等待你的决定".into(),
            allow: "允许".into(),
            deny: "拒绝".into(),
            needs_approval: "需要审批".into(),
            denied: "已拒绝".into(),
            awaiting_approval: "等待审批".into(),
            pending: "待处理".into(),
            running: "运行中".into(),
            completed: "已完成".into(),
            failed: "失败".into(),
            cancelled: "已取消".into(),
            unknown: "状态未知".into(),
        },
    }
}

/// Colours the Trajectory overview needs and the theme does not name.
///
/// The lanes borrow the shell's state colours instead of introducing a second
/// palette, so a tool block reads as the same "activity" colour as a running
/// tool card.
struct TrajectoryPalette {
    lanes: [gpui::Hsla; 3],
    rule: gpui::Hsla,
    selection: gpui::Hsla,
    accent: gpui::Hsla,
    error: gpui::Hsla,
}

impl TrajectoryPalette {
    fn new(cx: &Context<DesktopRoot>) -> Self {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let input = theme.success.opacity(if dark { 0.69 } else { 0.75 });
        let model = if dark {
            gpui::rgba(0xbf8bffc4).into()
        } else {
            gpui::rgba(0xa06be0c8).into()
        };
        let tools = theme.warning.opacity(if dark { 0.88 } else { 0.85 });
        Self {
            lanes: [input, model, tools],
            rule: theme.border,
            selection: theme.primary.opacity(0.12),
            accent: theme.primary,
            error: theme.danger,
        }
    }

    /// Fade a block that the live search filtered out.
    fn dim(&self, color: gpui::Hsla) -> gpui::Hsla {
        let mut faded = color;
        faded.a *= 0.22;
        faded
    }
}

/// Paints one rectangle, optionally with a border.
fn paint_rect(
    left: gpui::Pixels,
    top: gpui::Pixels,
    width: gpui::Pixels,
    height: gpui::Pixels,
    background: gpui::Hsla,
    radius: gpui::Pixels,
    border: Option<gpui::Hsla>,
) -> gpui::PaintQuad {
    gpui::quad(
        gpui::Bounds {
            origin: gpui::point(left, top),
            size: gpui::size(width, height),
        },
        radius,
        background,
        if border.is_some() { radius } else { px(0.) },
        border.unwrap_or_else(gpui::transparent_black),
        gpui::BorderStyle::Solid,
    )
}

fn section_label(label: &'static str, cx: &mut Context<DesktopRoot>) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// One Trajectory ledger item: a turn header plus the record beside it.
///
/// The row is deliberately monospaced and label-first — the ledger is evidence,
/// not prose — and every row activates the same selection the overview's block
/// does, so the two views drive one selection.
fn ledger_row_element(
    row: &trajectory_page::LedgerRow,
    selected: bool,
    weak: WeakEntity<DesktopRoot>,
    palette: LedgerPalette,
    language: tact_catalog::CatalogLocale,
) -> AnyElement {
    let id = row.index;
    let record_id = row.id;
    let kind = row.kind_label;
    let (kind_bg, kind_fg) = palette.kind(kind);
    let click_weak = weak.clone();
    let text = row.text.clone();
    let duration = row.duration.clone();
    let status = row.status;
    let failed = row.failed;
    let starts_turn = row.starts_turn;
    let turn = row.turn;
    v_flex()
        .id(SharedString::from(format!("trajectory-row-{id}")))
        .debug_selector(move || format!("trajectory-row-{id}"))
        .role(Role::ListItem)
        .aria_selected(selected)
        .aria_label(format!("#{id} {kind} {text}"))
        .w_full()
        .min_w_0()
        .when(starts_turn, |item| {
            item.child(
                h_flex()
                    .id(SharedString::from(format!("trajectory-turn-{id}")))
                    .debug_selector(move || format!("trajectory-turn-{id}"))
                    .w_full()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .pt(px(12.))
                    .pb(px(5.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_semibold()
                            .font_family("Lilex")
                            .text_color(palette.muted)
                            .child(format!(
                                "{} {}",
                                i18n::text(language, TextKey::TurnPrefix),
                                turn
                            )),
                    )
                    .child(div().flex_1().h(px(1.)).bg(palette.rule)),
            )
        })
        .child(
            h_flex()
                .id(SharedString::from(format!("trajectory-record-{id}")))
                .debug_selector(move || format!("trajectory-record-{id}"))
                .role(Role::Button)
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.bg(palette.selected_row))
                .aria_label(format!("#{id} {kind}"))
                .cursor_pointer()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(9.))
                .px(px(10.))
                .py(px(6.))
                .when(selected, |line| line.bg(palette.selected_row))
                .hover(|style| style.bg(palette.hover_row))
                .child(
                    div()
                        .flex_none()
                        .w(px(30.))
                        .text_size(px(10.))
                        .font_family("Lilex")
                        .text_color(palette.muted)
                        .child(format!("{id}")),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(74.))
                        .rounded(px(4.))
                        .bg(kind_bg)
                        .py(px(1.))
                        .text_size(px(9.5))
                        .font_semibold()
                        .font_family("Lilex")
                        .text_color(kind_fg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(kind),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(if failed { palette.danger } else { palette.ink })
                        .child(text),
                )
                .when_some(duration, |line, duration| {
                    line.child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(palette.muted)
                            .child(duration),
                    )
                })
                .when(failed, |line| {
                    line.child(
                        div()
                            .flex_none()
                            .rounded(px(3.))
                            .bg(palette.danger_chip)
                            .px(px(4.))
                            .py(px(1.))
                            .text_size(px(9.5))
                            .font_family("Lilex")
                            .text_color(palette.danger)
                            .child(status),
                    )
                })
                .on_click(move |_, _, cx: &mut gpui::App| {
                    let _ = click_weak.update(cx, |root, cx| {
                        root.select_trajectory_record(record_id, cx);
                    });
                }),
        )
        .into_any_element()
}

/// The ledger's colours, resolved from the theme once per frame.
///
/// `gpui::rgba` is RRGGBBAA, so every alpha here is written as a trailing
/// byte rather than OR-ed on: a mistaken `.into()` on `0xrrggbb22` would paint
/// a fully transparent tint and the chip would read as an empty box.
#[derive(Clone, Copy)]
struct LedgerPalette {
    ink: gpui::Hsla,
    muted: gpui::Hsla,
    danger: gpui::Hsla,
    user: (gpui::Hsla, gpui::Hsla),
    context: (gpui::Hsla, gpui::Hsla),
    assistant: (gpui::Hsla, gpui::Hsla),
    tool: (gpui::Hsla, gpui::Hsla),
    gate: (gpui::Hsla, gpui::Hsla),
    error: (gpui::Hsla, gpui::Hsla),
    other: (gpui::Hsla, gpui::Hsla),
    rule: gpui::Hsla,
    selected_row: gpui::Hsla,
    hover_row: gpui::Hsla,
    danger_chip: gpui::Hsla,
}

impl LedgerPalette {
    fn new(cx: &Context<DesktopRoot>) -> Self {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let ink = theme.foreground;
        let muted = theme.muted_foreground;
        // Tints are the state colours at ~13% and the label colours are the
        // same hue at full strength, so a chip stays legible in both themes.
        let chip = |tint: u32, label: u32, tint_alpha: u8| {
            (
                gpui::Hsla::from(gpui::rgba((tint << 8) | tint_alpha as u32)),
                gpui::Hsla::from(gpui::rgba((label << 8) | 0xff)),
            )
        };
        let (user, user_label) = (
            theme.primary.opacity(if dark { 0.22 } else { 0.13 }),
            theme.primary,
        );
        let (context, context_label) = if dark {
            chip(0x30d158, 0x63e07f, 0x38)
        } else {
            chip(0x30d158, 0x1f7a3a, 0x22)
        };
        let (assistant, assistant_label) = if dark {
            chip(0xbf8bff, 0xd0aaff, 0x38)
        } else {
            chip(0xa06be0, 0x6b41a8, 0x22)
        };
        let (tool, tool_label) = if dark {
            chip(0xff9f0a, 0xffc061, 0x38)
        } else {
            chip(0xf08a1e, 0xa45c06, 0x22)
        };
        let (gate, gate_label) = if dark {
            chip(0xff9f0a, 0xffd08a, 0x38)
        } else {
            chip(0xff9f0a, 0xa45c06, 0x26)
        };
        let (error, error_label) = if dark {
            chip(0xff453a, 0xff8a80, 0x38)
        } else {
            chip(0xff453a, 0xd70015, 0x22)
        };
        let (other, other_label) = if dark {
            chip(0x98989d, 0xd8d8dc, 0x30)
        } else {
            chip(0x8e8e93, 0x545458, 0x22)
        };
        Self {
            ink,
            muted,
            danger: theme.danger,
            user: (user, user_label),
            context: (context, context_label),
            assistant: (assistant, assistant_label),
            tool: (tool, tool_label),
            gate: (gate, gate_label),
            error: (error, error_label),
            other: (other, other_label),
            rule: theme.border,
            selected_row: theme.primary.opacity(if dark { 0.20 } else { 0.10 }),
            hover_row: if dark {
                gpui::rgba(0xffffff0f).into()
            } else {
                gpui::rgba(0x0000000a).into()
            },
            danger_chip: theme.danger.opacity(if dark { 0.22 } else { 0.12 }),
        }
    }

    /// The kind chip's tint and label colours.
    fn kind(&self, kind: &str) -> (gpui::Hsla, gpui::Hsla) {
        match kind {
            "USER" => self.user,
            "CONTEXT" => self.context,
            "ASSISTANT" => self.assistant,
            "TOOL" => self.tool,
            "GATE" => self.gate,
            "ERROR" => self.error,
            _ => self.other,
        }
    }
}

fn transcript_row_element(
    row: &TranscriptRow,
    snapshot: &DesktopSnapshot,
    weak: WeakEntity<DesktopRoot>,
    assistant_selection: Option<Entity<SelectionActions>>,
    read_only: bool,
    gate_focus: Option<&gpui::FocusHandle>,
    thinking_open: bool,
) -> AnyElement {
    let session_id = snapshot
        .session
        .id
        .clone()
        .unwrap_or_else(|| SessionId(format!("epoch-{}", snapshot.session.epoch.0)));
    if !crate::ui::transcript::visible_row(row, snapshot) {
        return div().into_any_element();
    }
    let id = row.ordinal;
    let row_id = row.id;
    let content = match &row.body {
        RowBody::User(user) => h_flex()
            .id(SharedString::from(format!("row-user-{id}")))
            .debug_selector(move || format!("transcript-row-user-{id}"))
            .role(Role::Article)
            .aria_label(i18n::text(snapshot.language, TextKey::UserMessage))
            .w_full()
            .min_w_0()
            .items_start()
            // The reader's own words sit on the right, like a chat bubble;
            // the mark moves to the trailing edge with them.
            .justify_end()
            .gap_3()
            // Keep the trailing mark clear of the scrollbar.
            .py(px(18.))
            .pr(px(10.))
            .child(
                v_flex()
                    .flex_1()
                    .max_w(px(620.))
                    .min_w_0()
                    .items_end()
                    .gap_1()
                    .child(
                        h_flex().items_baseline().gap(px(7.)).child(
                            div()
                                .text_size(px(11.))
                                .font_semibold()
                                .text_color(snapshot_ink(snapshot))
                                .child(i18n::text(snapshot.language, TextKey::You)),
                        ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("user-message-{id}")))
                            .debug_selector(move || format!("user-message-{id}"))
                            .min_w_0()
                            .rounded(px(12.))
                            .border_1()
                            .border_color(snapshot_row_border(snapshot))
                            .bg(snapshot_row_surface(snapshot))
                            .px(px(14.))
                            .py(px(10.))
                            .text_sm()
                            .child(TextView::markdown(
                                format!("user-{id}"),
                                user.text.to_string(),
                            )),
                    )
                    .when(!user.attachments.is_empty(), |row| {
                        row.child(
                            AttachmentStrip::new(format!("user-attachments-{id}"))
                                .label(i18n::text(snapshot.language, TextKey::Attachment))
                                .items(user.attachments.iter().map(|path| {
                                    let mut item = AttachmentItem::new(
                                        path.display().to_string(),
                                        path.file_name().and_then(|name| name.to_str()).unwrap_or(
                                            i18n::text(snapshot.language, TextKey::Attachment),
                                        ),
                                    );
                                    if is_image_path(path) {
                                        item = item.image(path.clone());
                                    }
                                    item.size_bytes(
                                        std::fs::metadata(path)
                                            .map(|metadata| metadata.len())
                                            .unwrap_or(0),
                                    )
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .mt(px(4.))
                            .gap(px(8.))
                            .when_some(row.time.clone(), |line, time| {
                                line.child(
                                    div()
                                        .debug_selector(move || format!("user-time-{id}"))
                                        .text_size(px(10.))
                                        .font_family("Lilex")
                                        .text_color(snapshot_row_muted(snapshot))
                                        .child(time.to_string()),
                                )
                            })
                            .child(
                                gpui_base::Button::new(("copy-user", id))
                                    .debug_selector(move || format!("copy-user-{id}"))
                                    .role(Role::Button)
                                    .aria_label(i18n::text(snapshot.language, TextKey::CopyMessage))
                                    .cursor_pointer()
                                    .rounded(px(4.))
                                    .p(px(3.))
                                    .focus(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .hover(|style| style.bg(snapshot_row_border(snapshot)))
                                    .child(
                                        Icon::new(IconName::Copy)
                                            .xsmall()
                                            .text_color(snapshot_row_muted(snapshot)),
                                    )
                                    .on_click({
                                        let text = user.text.to_string();
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                text.clone(),
                                            ));
                                        }
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded(px(8.))
                    .bg(snapshot_ink(snapshot))
                    .text_size(px(10.))
                    .font_family("Lilex")
                    .font_semibold()
                    .text_color(snapshot_canvas(snapshot))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(local_user_initials()),
            )
            .into_any_element(),
        RowBody::Assistant(assistant) => {
            let display_markdown =
                assistant_display_markdown(&assistant.markdown, snapshot.project.root.as_deref());
            let diagrams: Vec<mermaid::Diagram> = mermaid::blocks(&assistant.markdown)
                .iter()
                .filter_map(|source| mermaid::parse(source))
                .collect();
            let content = streamed_text(&display_markdown, assistant.status, snapshot.language);
            let body = match (assistant.status, assistant_selection) {
                (RowStatus::Running, _) | (_, None) => {
                    StreamingText::new(format!("assistant-{id}"), &content)
                        .label(i18n::text(snapshot.language, TextKey::AssistantResponse))
                        .into_any_element()
                }
                (_, Some(selection)) => selection.into_any_element(),
            };
            h_flex()
                .id(SharedString::from(format!("row-assistant-{id}")))
                .debug_selector(move || format!("transcript-row-assistant-{id}"))
                .role(Role::Article)
                .aria_label(i18n::text(snapshot.language, TextKey::AssistantResponse))
                .items_start()
                .gap_3()
                .py_3()
                .child(
                    div()
                        .flex_none()
                        .size(px(28.))
                        .rounded(px(8.))
                        .bg(snapshot_row_surface(snapshot))
                        .border_1()
                        .border_color(snapshot_row_border(snapshot))
                        .text_color(snapshot_accent(snapshot))
                        .text_size(px(16.))
                        .font_family("SF Pro Display")
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("T"),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_baseline()
                                .gap(px(7.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_semibold()
                                        .text_color(snapshot_ink(snapshot))
                                        .child("Tact"),
                                )
                                .when_some(row.time.clone(), |line, time| {
                                    line.child(
                                        div()
                                            .text_size(px(10.))
                                            .font_family("Lilex")
                                            .text_color(snapshot_row_muted(snapshot))
                                            .child(time.to_string()),
                                    )
                                })
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_family("Lilex")
                                        .text_color(snapshot_row_muted(snapshot))
                                        .child(format!(
                                            "· {}",
                                            i18n::text(snapshot.language, TextKey::Assistant)
                                        )),
                                ),
                        )
                        .child(body)
                        .when(!diagrams.is_empty(), |row| {
                            let mut row = row;
                            for (index, diagram) in diagrams.iter().enumerate() {
                                row = row.child(mermaid::element(
                                    format!("assistant-mermaid-{id}-{index}"),
                                    diagram,
                                    snapshot.appearance == Appearance::Dark,
                                ));
                            }
                            row
                        })
                        .when(!read_only && !assistant.suggestions.is_empty(), |row| {
                            let suggestions = assistant.suggestions.clone();
                            let weak = weak.clone();
                            row.child(
                                Suggestions::new(format!("suggestions-{id}"))
                                    .label(i18n::text(snapshot.language, TextKey::Suggestions))
                                    .items(assistant.suggestions.iter().enumerate().map(
                                        |(index, suggestion)| {
                                            Suggestion::new(
                                                index.to_string(),
                                                suggestion.to_string(),
                                            )
                                        },
                                    ))
                                    .on_event(move |event, window, cx| {
                                        let SuggestionsEvent::Selected { id } = event;
                                        let Some(suggestion) = id
                                            .parse::<usize>()
                                            .ok()
                                            .and_then(|index| suggestions.get(index))
                                        else {
                                            return;
                                        };
                                        let suggestion = suggestion.to_string();
                                        let _ = weak.update(cx, |this, cx| {
                                            this.prompt.update(cx, |prompt, cx| {
                                                prompt.set_draft(suggestion, window, cx);
                                                prompt.focus(window, cx);
                                            });
                                            cx.notify();
                                        });
                                    }),
                            )
                        }),
                )
                .into_any_element()
        }
        RowBody::Plan(plan) => {
            // A plan made only of tool calls would draw every command twice:
            // once here and once in the execution group below. The tool rows
            // carry the status, output and disclosure, so the plan card yields
            // when every step is already represented in this run.
            if plan_is_tool_execution(row, plan, &snapshot.transcript) {
                return div().into_any_element();
            }
            let steps = plan.steps.iter().enumerate().map(|(index, step)| {
                let tone = match step.status {
                    RowStatus::Failed => snapshot_theme_danger(snapshot),
                    RowStatus::Pending | RowStatus::Cancelled => snapshot_row_muted(snapshot),
                    _ => snapshot_ink(snapshot),
                };
                // The design marks state in the first column — a check for a
                // done step, a ringed dot for the running one — and keeps the
                // ordinal at the row's right edge.
                let mark = match step.status {
                    RowStatus::Success | RowStatus::Resolved => div()
                        .flex_none()
                        .size(px(17.))
                        .rounded_full()
                        .border_1()
                        .border_color(snapshot_theme_primary(snapshot))
                        .bg(snapshot_theme_primary(snapshot).opacity(0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Check)
                                .size(px(10.))
                                .text_color(snapshot_theme_primary(snapshot)),
                        ),
                    RowStatus::Running => div()
                        .flex_none()
                        .size(px(17.))
                        .rounded_full()
                        .border_1()
                        .border_color(snapshot_theme_primary(snapshot))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(5.))
                                .rounded_full()
                                .bg(snapshot_theme_primary(snapshot)),
                        ),
                    RowStatus::Failed => div()
                        .flex_none()
                        .size(px(17.))
                        .rounded_full()
                        .border_1()
                        .border_color(snapshot_theme_danger(snapshot))
                        .bg(snapshot_theme_danger(snapshot).opacity(0.12)),
                    _ => div()
                        .flex_none()
                        .size(px(17.))
                        .rounded_full()
                        .border_1()
                        .border_color(snapshot_row_border(snapshot)),
                };
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(9.))
                    .px(px(17.))
                    .min_h(px(38.))
                    .when(index + 1 < plan.steps.len(), |row| {
                        row.border_b_1().border_color(snapshot_row_border(snapshot))
                    })
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(tone)
                            .child(step.label.to_string()),
                    )
                    .when_some(step.detail.clone(), |row, detail| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .font_family("Lilex")
                                .text_color(snapshot_row_muted(snapshot))
                                .child(detail.to_string()),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(22.))
                            .text_right()
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(snapshot_row_muted(snapshot))
                            .child(format!("{:02}", index + 1)),
                    )
            });
            let executing = plan
                .steps
                .iter()
                .any(|step| matches!(step.status, RowStatus::Running | RowStatus::Pending));
            v_flex()
                .id(SharedString::from(format!("row-plan-{id}")))
                .debug_selector(move || format!("transcript-row-plan-{id}"))
                .role(Role::Group)
                .gap(px(0.))
                .rounded(px(12.))
                .border_1()
                .border_color(snapshot_row_border(snapshot))
                .bg(snapshot_row_surface(snapshot))
                .shadow_sm()
                .overflow_hidden()
                .child(
                    v_flex()
                        .gap(px(3.))
                        .px(px(17.))
                        .pt(px(16.))
                        .pb(px(13.))
                        .border_b_1()
                        .border_color(snapshot_row_border(snapshot))
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_family("Lilex")
                                        .text_color(snapshot_row_muted(snapshot))
                                        .child(plan_step_label(
                                            plan.steps.len(),
                                            snapshot.language,
                                        )),
                                )
                                .child(
                                    h_flex()
                                        .gap(px(5.))
                                        .items_center()
                                        .rounded_full()
                                        .bg(snapshot_theme_success(snapshot).opacity(0.14))
                                        .px(px(7.))
                                        .py(px(2.))
                                        .child(
                                            div()
                                                .size(px(5.))
                                                .rounded_full()
                                                .bg(snapshot_theme_success(snapshot)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .font_family("Lilex")
                                                .text_color(snapshot_theme_success(snapshot))
                                                .child(plan_status_label(
                                                    executing,
                                                    snapshot.language,
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(19.))
                                .font_semibold()
                                .child(plan.title.to_string()),
                        ),
                )
                .child(v_flex().children(steps))
                .into_any_element()
        }
        RowBody::Thinking(thinking) => {
            let reader_weak = weak.clone();
            v_flex()
                .id(SharedString::from(format!("row-thinking-{id}")))
                .debug_selector(move || format!("transcript-row-thinking-{id}"))
                .child(
                    Thinking::new(format!("thinking-{id}"), thinking.markdown.to_string())
                        .status(thinking.status)
                        .duration_ms(thinking.duration_ms)
                        .labels(thinking_labels(snapshot.language))
                        // The trace expands in place. It opens on its own
                        // while reasoning runs and afterwards stays where the
                        // reader left it; no reader overlay is involved.
                        .open(
                            thinking_open
                                || matches!(
                                    thinking.status,
                                    RowStatus::Running | RowStatus::Pending
                                ),
                        )
                        .on_event(move |event, _, cx| {
                            let ThinkingEvent::Toggled { open, .. } = event;
                            let open = *open;
                            let thinking_key = (session_id.clone(), row_id);
                            let _ = reader_weak.update(cx, |this, cx| {
                                if open {
                                    this.expanded_thinking.insert(thinking_key.clone());
                                } else {
                                    this.expanded_thinking.remove(&thinking_key);
                                }
                                this.pending_row_updates.insert(row_id);
                                cx.notify();
                            });
                        }),
                )
                .into_any_element()
        }
        RowBody::ToolGroup(group) => {
            let tool_count = group
                .items
                .iter()
                .filter(|item| matches!(item, ToolGroupItem::Tool(_)))
                .count();
            let mut gate_focus_used = false;
            let tools = group
                .items
                .iter()
                .map(|item| {
                    let preview = match item {
                        ToolGroupItem::Gate(gate) => gate_tool_preview(group, gate),
                        ToolGroupItem::Tool(_) => None,
                    };
                    // A group owns one focus handle; only the first gate in
                    // it can claim it, or two elements would track one handle.
                    let item_focus = if matches!(item, ToolGroupItem::Gate(_)) && !gate_focus_used {
                        gate_focus_used = true;
                        gate_focus
                    } else {
                        None
                    };
                    tool_group_item_element(
                        item,
                        row.id,
                        snapshot,
                        weak.clone(),
                        read_only,
                        preview,
                        item_focus,
                    )
                })
                .collect::<Vec<_>>();
            let group_id = row.id;
            let group_weak = weak.clone();
            let active = matches!(group.status(), RowStatus::Running | RowStatus::Pending);
            let quiet = group.items.len() == 1 && group.collapsed && !active;
            div()
                .debug_selector(move || format!("transcript-row-tool-group-{id}"))
                // The design makes the group the card: one border, one
                // radius, and the calls as rows inside it.
                .w_full()
                .min_w_0()
                .when(quiet, |card| card.rounded(px(6.)))
                .when(!quiet, |card| {
                    card.rounded(px(12.))
                        .border_1()
                        .border_color(snapshot_row_border(snapshot))
                        .bg(snapshot_row_surface(snapshot))
                        .shadow_sm()
                        .overflow_hidden()
                })
                .child(
                    ToolGroup::new(format!("tool-group-{id}"))
                        .title(tool_group_title(group, snapshot.language))
                        .count(tool_count)
                        .compact()
                        .quiet(quiet)
                        .active(active)
                        .open(!group.collapsed)
                        .on_event(move |event, _, cx| {
                            if matches!(event, ToolGroupEvent::Toggled { .. }) {
                                let _ = group_weak.update(cx, |this, _| {
                                    this.dispatch(DesktopIntent::ToggleGroup { id: group_id });
                                });
                            }
                        })
                        .children(tools),
                )
                .into_any_element()
        }
        RowBody::Diff(diff) => {
            let rendered = DiffFile::from_unified(&diff.patch)
                .into_iter()
                .next()
                .map(|file| {
                    CodeDiff::new(format!("diff-{id}"), &file)
                        .open(true)
                        .into_any_element()
                })
                .unwrap_or_else(|| {
                    CodeBlock::new(format!("diff-{id}"), diff.patch.clone())
                        .language("diff")
                        .into_any_element()
                });
            v_flex()
                .id(SharedString::from(format!("row-diff-{id}")))
                .debug_selector(move || format!("transcript-row-diff-{id}"))
                .role(Role::Region)
                .aria_label(i18n::text(snapshot.language, TextKey::Diff))
                .gap_1()
                .child(
                    div().text_xs().child(
                        diff.path
                            .as_deref()
                            .unwrap_or(i18n::text(snapshot.language, TextKey::Diff))
                            .to_string(),
                    ),
                )
                .child(rendered)
                .child(
                    div()
                        .id(SharedString::from(format!("open-diff-{id}")))
                        .role(Role::Button)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                        .aria_label(i18n::text(snapshot.language, TextKey::OpenDiff))
                        .cursor_pointer()
                        .text_xs()
                        .child(i18n::text(snapshot.language, TextKey::OpenDiff))
                        .on_click({
                            let weak = weak.clone();
                            move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.remember_focus(window, cx);
                                    this.open_transcript_reader(ReaderRef::Diff(row_id), cx);
                                });
                            }
                        }),
                )
                .into_any_element()
        }
        RowBody::Gate(gate) => div()
            .debug_selector(move || format!("transcript-row-gate-{id}"))
            .child(gate_element(
                gate, snapshot, weak, read_only, None, gate_focus,
            ))
            .into_any_element(),
        RowBody::Notice(notice) => div()
            .id(SharedString::from(format!("row-notice-{id}")))
            .debug_selector(move || format!("transcript-row-notice-{id}"))
            .role(Role::Status)
            .p_1()
            .text_sm()
            .child(notice.text.to_string())
            .into_any_element(),
        RowBody::HookContext(context) => {
            let source = context
                .source
                .as_deref()
                .unwrap_or(i18n::text(snapshot.language, TextKey::Hooks));
            let title = hook_context_title(&context.markdown, source);
            let summary = hook_context_summary(&context.markdown, &title);
            let reader_weak = weak.clone();
            let reader_row = row_id;
            let primary = snapshot_theme_primary(snapshot);
            let aria_label = if title == source {
                source.to_string()
            } else {
                format!("{source} · {title}")
            };
            h_flex()
                .id(SharedString::from(format!("row-hook-context-{id}")))
                .debug_selector(move || format!("transcript-row-hook-context-{id}"))
                .role(Role::Button)
                .aria_label(aria_label)
                .when(!summary.is_empty(), |card| {
                    card.aria_description(summary.clone())
                })
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.border_color(primary))
                .cursor_pointer()
                .w_full()
                .min_w_0()
                .items_center()
                .gap_3()
                .rounded(px(10.))
                .border_1()
                .border_color(snapshot_row_border(snapshot))
                .bg(snapshot_row_background(snapshot))
                .px_3()
                .py_2()
                .hover(|style| style.border_color(primary.opacity(0.45)))
                .on_click(move |_, window, cx| {
                    let _ = reader_weak.update(cx, |this, cx| {
                        if let Some((title, markdown)) = this.hook_context_reader(reader_row) {
                            this.remember_focus(window, cx);
                            this.open_overlay(Overlay::Reader(ReaderState::new(title, markdown)));
                            cx.notify();
                        }
                    });
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(28.))
                        .rounded(px(8.))
                        .bg(primary.opacity(0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::BookOpen).xsmall().text_color(primary)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(220.))
                                        .truncate()
                                        .font_family("Lilex")
                                        .text_size(px(10.))
                                        .text_color(snapshot_row_muted(snapshot))
                                        .child(source.to_string()),
                                )
                                .when(title != source, |line| {
                                    line.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .font_semibold()
                                            .child(title),
                                    )
                                }),
                        )
                        .when(!summary.is_empty(), |body| {
                            body.child(
                                div()
                                    .text_xs()
                                    .text_color(snapshot_row_muted(snapshot))
                                    .line_clamp(2)
                                    .child(summary),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(primary)
                        .child(i18n::text(snapshot.language, TextKey::Open))
                        .child(Icon::new(IconName::ExternalLink).xsmall()),
                )
                .into_any_element()
        }
        RowBody::HookStatus(status) => div()
            .id(SharedString::from(format!("row-hook-status-{id}")))
            .debug_selector(move || format!("transcript-row-hook-status-{id}"))
            .role(Role::Status)
            .text_xs()
            .child(format!(
                "{} · {}{}",
                status
                    .source
                    .as_deref()
                    .unwrap_or(i18n::text(snapshot.language, TextKey::Hooks)),
                status.message,
                status
                    .elapsed_ms
                    .map(|elapsed| format!(" · {elapsed} ms"))
                    .unwrap_or_default()
            ))
            .into_any_element(),
        RowBody::Error(error) => v_flex()
            .id(SharedString::from(format!("row-error-{id}")))
            .debug_selector(move || format!("transcript-row-error-{id}"))
            .role(Role::Alert)
            .gap_1()
            .border_1()
            .rounded_md()
            .p_2()
            .child(
                div()
                    .font_semibold()
                    .child(i18n::text(snapshot.language, TextKey::Error)),
            )
            .child(TextView::markdown(
                format!("error-{id}"),
                error.message.to_string(),
            ))
            .when(!read_only && error.retryable, |card| {
                card.child(
                    div()
                        .id(SharedString::from(format!("retry-error-{id}")))
                        .role(Role::Button)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                        .aria_label(i18n::text(snapshot.language, TextKey::Retry))
                        .text_xs()
                        .text_color(gpui::blue())
                        .cursor_pointer()
                        .child(i18n::text(snapshot.language, TextKey::Retry))
                        .on_click({
                            let weak = weak.clone();
                            move |_, _, cx| {
                                let _ = weak.update(cx, |this, _| {
                                    this.dispatch(DesktopIntent::RetryProvider);
                                });
                            }
                        }),
                )
            })
            .into_any_element(),
    };
    // User and assistant rows draw their own avatar in the first column; every
    // other row is content that belongs to that column and starts where the
    // message text does. Without this inset a plan card runs the full column
    // while the answer above it is indented, which is the misalignment the
    // design's row grid exists to prevent.
    let avatar_gutter = row_starts_at_message_column(&row.body);
    v_flex()
        .id(SharedString::from(format!("row-a11y-{id}")))
        .role(crate::ui::transcript_row::accessibility_role(row))
        .aria_label(crate::ui::transcript_row::accessibility_label(
            row,
            snapshot.language,
        ))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_stretch()
                .when(avatar_gutter, |line| {
                    line.child(transcript_gutter(row, snapshot))
                })
                .child(div().flex_1().min_w_0().child(content)),
        )
        .into_any_element()
}

/// The decorative mark in the avatar column for rows without an avatar.
///
/// The design keeps the column: a thinking row carries `∴` and a tool group
/// its vertical rail and knot. A gate draws its own signal inside its card.
fn transcript_gutter(row: &TranscriptRow, snapshot: &DesktopSnapshot) -> AnyElement {
    let mark = match &row.body {
        RowBody::Thinking(_) => Some(
            div()
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(16.))
                .text_color(snapshot_row_muted(snapshot))
                .child("∴"),
        ),
        RowBody::ToolGroup(_) => Some(
            v_flex()
                .w(px(28.))
                .items_center()
                .pt(px(5.))
                .child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .border_1()
                        .border_color(snapshot_row_border(snapshot))
                        .bg(snapshot_row_surface(snapshot)),
                )
                .child(div().w(px(1.)).flex_1().bg(snapshot_row_border(snapshot))),
        ),
        RowBody::Gate(_) => None,
        _ => None,
    };
    h_flex()
        .flex_none()
        .w(px(40.))
        .when_some(mark, |gutter, mark| gutter.child(mark))
        .into_any_element()
}

/// The command a gate is asking about, when the group also holds its tool.
///
/// The design repeats the command inside the permission card; the tool row is
/// the only place the desktop transcript carries it.
fn gate_tool_preview(
    group: &tact_desktop_core::ToolGroupRow,
    gate: &GateCard,
) -> Option<SharedString> {
    let tool_id = match &gate.origin {
        tact_desktop_core::GateOrigin::Tool { tool_id } => Some(&**tool_id),
        tact_desktop_core::GateOrigin::Subagent {
            tool_id: Some(tool_id),
            ..
        } => Some(&**tool_id),
        _ => None,
    }?;
    group.items.iter().find_map(|item| match item {
        ToolGroupItem::Tool(tool) if &*tool.tool_id == tool_id => Some(SharedString::from(
            tool.arg_full.as_deref().unwrap_or(&*tool.arg_summary),
        )),
        _ => None,
    })
}

/// The first line of a gate prompt.
///
/// Command prompts embed the whole script after the first line; the preview
/// block and the Review surface carry the rest, so a prompt row never grows
/// with the command it quotes.
fn gate_prompt_line(prompt: &str) -> &str {
    prompt.lines().next().unwrap_or_default().trim()
}

/// The command a gate prompt quotes in backticks, when the group has no tool
/// to borrow it from.
fn gate_prompt_command(prompt: &str) -> Option<SharedString> {
    let start = prompt.find('`')?;
    let rest = &prompt[start + 1..];
    let end = rest.find('`')?;
    (!rest[..end].trim().is_empty()).then(|| SharedString::from(rest[..end].trim()))
}

/// The tool that raised a gate, when the gate carries a tool identity.
fn tool_name_for_id<'a>(
    view: &'a tact_desktop_core::TranscriptView,
    tool_id: &str,
) -> Option<&'a str> {
    let mut found = None;
    for row in view.iter() {
        let RowBody::ToolGroup(group) = &row.body else {
            continue;
        };
        for item in group.items.iter() {
            if let ToolGroupItem::Tool(tool) = item
                && tool.tool_id.as_ref() == tool_id
            {
                found = Some(tool.name.as_ref());
            }
        }
    }
    found
}

fn gate_tool_name<'a>(snapshot: &'a DesktopSnapshot, gate: &GateCard) -> Option<&'a str> {
    match &gate.origin {
        tact_desktop_core::GateOrigin::Tool { tool_id } => {
            tool_name_for_id(&snapshot.transcript, tool_id)
        }
        tact_desktop_core::GateOrigin::Subagent {
            child_id,
            tool_id: Some(tool_id),
        } => {
            // A child tool lives in the spawn card's nested transcript, not in
            // the parent's top-level rows.
            let mut found = None;
            for row in snapshot.transcript.iter() {
                let RowBody::ToolGroup(group) = &row.body else {
                    continue;
                };
                for item in group.items.iter() {
                    let ToolGroupItem::Tool(spawn) = item else {
                        continue;
                    };
                    if spawn.meta.subagent_id.as_deref() == Some(child_id.as_ref())
                        && let Some(nested) = &spawn.subagent
                        && let Some(name) = tool_name_for_id(nested, tool_id)
                    {
                        found = Some(name);
                    }
                }
            }
            found
        }
        _ => None,
    }
}

/// Help text under a gate title. A command, a question, and a subagent are
/// different promises; the command wording must not leak onto the others.
fn gate_description(
    locale: tact_catalog::CatalogLocale,
    tool_name: Option<&str>,
) -> Option<&'static str> {
    match tool_name {
        Some("bash" | "worktree_run" | "background_run") => Some(match locale {
            tact_catalog::CatalogLocale::En => {
                "This command runs in the current worktree and may write files."
            }
            tact_catalog::CatalogLocale::Zh => "该命令会在当前工作树中运行，并可能写入文件。",
        }),
        Some("ask_user") => Some(match locale {
            tact_catalog::CatalogLocale::En => {
                "This asks you a question and waits for your answer."
            }
            tact_catalog::CatalogLocale::Zh => "这会向你提问，并等待你的回答。",
        }),
        Some("spawn_subagent") => Some(match locale {
            tact_catalog::CatalogLocale::En => "This runs a subagent in its own session.",
            tact_catalog::CatalogLocale::Zh => "这会在独立会话中运行一个子代理。",
        }),
        _ => None,
    }
}

fn tool_group_item_element(
    item: &ToolGroupItem,
    group_id: RowId,
    snapshot: &DesktopSnapshot,
    weak: WeakEntity<DesktopRoot>,
    read_only: bool,
    preview: Option<SharedString>,
    gate_focus: Option<&gpui::FocusHandle>,
) -> AnyElement {
    match item {
        ToolGroupItem::Gate(gate) => {
            gate_element(gate, snapshot, weak, read_only, preview, gate_focus)
        }
        ToolGroupItem::Tool(tool) => tool_row_element(tool, group_id, snapshot.language, weak),
    }
}

fn reader_markdown_element(
    reader: &mut ReaderState,
    window: &mut Window,
    cx: &mut Context<DesktopRoot>,
) -> AnyElement {
    let selection = match reader.selection.clone() {
        Some(selection) => selection,
        None => {
            let markdown = reader.markdown.clone();
            let selection = cx.new(|cx| {
                SelectionActions::new("reader-selection", markdown, window, cx)
                    .frameless()
                    .natural_height()
            });
            reader.selection = Some(selection.clone());
            selection
        }
    };
    selection.update(cx, |selection, cx| {
        selection.set_markdown(reader.markdown.clone(), cx);
    });
    let markdown_view = match reader.markdown_view.clone() {
        Some(view) => view,
        None => {
            let view = cx.new(|_| ReaderMarkdownView {
                selection,
                scroll: ScrollHandle::new(),
                pending_scroll: Rc::new(Cell::new(0.)),
                scroll_scheduled: Rc::new(Cell::new(false)),
                scroll_task: Rc::new(RefCell::new(None)),
            });
            reader.markdown_view = Some(view.clone());
            view
        }
    };
    markdown_view.into_any_element()
}

fn tool_row_element(
    tool: &ToolRow,
    group_id: RowId,
    locale: tact_catalog::CatalogLocale,
    weak: WeakEntity<DesktopRoot>,
) -> AnyElement {
    let output = tool_body_markdown(tool, locale);
    let mut call = ToolCall::new(tool.tool_id.to_string(), tool.name.to_string())
        .summary(tact_desktop_core::one_line_summary(&tool.arg_summary))
        .input(tool.arg_full.as_deref().unwrap_or(&tool.arg_summary))
        .status(tool.status)
        .labels(tool_call_labels(locale))
        .grouped()
        .open(tool.expanded);
    if !output.is_empty() {
        call = call.output(output);
    }
    if tool.name.as_ref() == "read_image"
        && let Some(image) = read_image_output(tool_output_text(tool))
        && image.path.is_file()
        && is_image_path(&image.path)
    {
        call = call.image_path(image.path);
    }
    let call_weak = weak.clone();
    let reader_weak = weak;
    let tool_id = tool.id;
    let mut row = v_flex().gap_1().child(call.on_event(move |event, _, cx| {
        if matches!(event, ToolCallEvent::Toggled { .. }) {
            let _ = call_weak.update(cx, |this, _| {
                this.dispatch(DesktopIntent::ToggleTool {
                    group: group_id,
                    tool: tool_id,
                });
            });
        }
    }));
    if !tool.output.preview.is_empty() || tool.output.detail.is_some() {
        row = row.child(
            div()
                .id(SharedString::from(format!("open-tool-{tool_id:?}")))
                .role(Role::Button)
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                .aria_label(i18n::text(locale, TextKey::OpenOutput))
                .cursor_pointer()
                .text_xs()
                .child(i18n::text(locale, TextKey::OpenOutput))
                .on_click(move |_, window, cx| {
                    let _ = reader_weak.update(cx, |this, cx| {
                        this.remember_focus(window, cx);
                        this.open_transcript_reader(ReaderRef::ToolOutput(tool_id), cx);
                    });
                }),
        );
    }
    row.into_any_element()
}

/// The first readable line of retained evidence, bounded for a card.
fn evidence_snippet(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .trim_start_matches('#')
        .trim();
    let line = line.replace("**", "").replace('`', "");
    let mut chars = line.chars();
    let snippet: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        format!("{snippet}…")
    } else {
        snippet
    }
}

/// The heading a plugin context uses as its card title, or its source.
fn hook_context_title(markdown: &str, fallback: &str) -> String {
    let title = markdown
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('#'))
        .map(compact_markdown_line)
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    truncate_display_text(&title, 72)
}

/// A bounded plain-text preview. The full Markdown stays behind the reader.
fn hook_context_summary(markdown: &str, title: &str) -> String {
    let mut parts = Vec::new();
    for line in markdown.lines().map(str::trim) {
        if line.is_empty()
            || line.starts_with("```")
            || line.chars().all(|ch| matches!(ch, '-' | '*' | '_'))
        {
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let line = compact_markdown_line(line);
        if line.is_empty() || line == title {
            continue;
        }
        parts.push(truncate_display_text(&line, 160));
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" · ")
}

fn compact_markdown_line(line: &str) -> String {
    line.trim()
        .trim_start_matches(['#', '>', '-', '*', '+', ' '])
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

/// Builds the `DiffTable` rows for a multi-file patch.
fn diff_rows(files: &[diff_review::FileDiff], decisions: &[DiffProposalState]) -> Arc<[DiffRow]> {
    let rows: Arc<[DiffRow]> = files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            DiffRow::new(
                format!("file-{index}"),
                file.path.clone(),
                DiffChangeKind::Changed,
            )
            .cells([
                DiffCell::unchanged("file", file.path.clone()),
                DiffCell::unchanged("added", file.added.to_string()),
                DiffCell::unchanged("removed", file.removed.to_string()),
            ])
            .state(
                decisions
                    .get(index)
                    .copied()
                    .unwrap_or(DiffProposalState::Pending),
            )
        })
        .collect::<Vec<_>>()
        .into();
    rows
}

fn tool_body_markdown(tool: &ToolRow, locale: tact_catalog::CatalogLocale) -> String {
    if tool.name.as_ref() == "read_image"
        && let Some(image) = read_image_output(tool_output_text(tool))
    {
        return image.summary;
    }
    let body = match &tool.body {
        tact_desktop_core::ToolBody::Code { language, text } => {
            let original = language.as_deref().unwrap_or("text");
            let language = crate::ui::transcript_row::evidence_language(original);
            let notice = if original.eq_ignore_ascii_case("mermaid") {
                format!("{}\n\n", i18n::text(locale, TextKey::MermaidFallback))
            } else if original.eq_ignore_ascii_case("ansi") {
                format!("{}\n\n", i18n::text(locale, TextKey::AnsiFallback))
            } else {
                String::new()
            };
            format!("{notice}```{language}\n{}\n```", text)
        }
        tact_desktop_core::ToolBody::Diff { path, patch } => format!(
            "### {}\n\n```diff\n{}\n```",
            path.as_deref().unwrap_or(i18n::text(locale, TextKey::Diff)),
            patch
        ),
        tact_desktop_core::ToolBody::Text(text) => text.to_string(),
        tact_desktop_core::ToolBody::None => tool
            .output
            .detail
            .as_deref()
            .unwrap_or(&tool.output.preview)
            .to_string(),
    };
    if tool.output.truncated && !body.is_empty() {
        let notice = i18n::text(locale, TextKey::ToolOutputTruncated);
        format!("_{notice}_\n\n{body}")
    } else {
        body
    }
}

fn tool_output_text(tool: &ToolRow) -> &str {
    tool.output
        .detail
        .as_deref()
        .unwrap_or(&tool.output.preview)
}

/// Assistant prose keeps its original Markdown; only the display copy turns
/// existing absolute paths into compact, clickable file links.
fn assistant_display_markdown(markdown: &str, root: Option<&Path>) -> String {
    let stripped = mermaid::strip_blocks(markdown);
    file_links::linkify_paths(&stripped, root)
}

/// The structurally useful parts of a `read_image` envelope. The base64 image
/// block is never read here; the original envelope stays in the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadImageOutput {
    path: PathBuf,
    summary: String,
}

/// `read_image` returns a small XML-ish envelope whose blank lines used to
/// render as a large empty output box. Keep the transcript line compact; the
/// reader still opens the untouched envelope.
fn read_image_output(text: &str) -> Option<ReadImageOutput> {
    let path = text.split_once("<path>")?.1.split_once("</path>")?.0.trim();
    let metadata = text
        .lines()
        .find(|line| line.contains("image/") && line.contains(" px,"))?
        .trim()
        .replace(" image, ", " · ")
        .replace(" px, ", " px · ");
    if path.is_empty() || metadata.is_empty() {
        return None;
    }
    Some(ReadImageOutput {
        path: PathBuf::from(path),
        summary: format!("`{path}` · {metadata}"),
    })
}

fn streamed_text(
    markdown: &str,
    status: RowStatus,
    locale: tact_catalog::CatalogLocale,
) -> StreamedContent {
    let mut content = StreamedContent::done(markdown.to_string());
    match status {
        RowStatus::Pending => {
            content = StreamedContent::pending(markdown.to_string());
        }
        RowStatus::Unknown => {
            content = StreamedContent::unknown(markdown.to_string());
        }
        RowStatus::Running => {
            content = StreamedContent::running(markdown.to_string());
        }
        RowStatus::Cancelled => {
            content = StreamedContent::cancelled(markdown.to_string());
        }
        RowStatus::Failed => {
            content = StreamedContent::failed(
                markdown.to_string(),
                i18n::text(locale, TextKey::ResponseFailed),
            );
        }
        RowStatus::Success | RowStatus::Resolved => {}
    }
    content
}

#[cfg(test)]
fn progress_state(status: RowStatus, locale: tact_catalog::CatalogLocale) -> ProgressState {
    match status {
        RowStatus::Pending => ProgressState::Pending,
        RowStatus::Running => ProgressState::Running,
        RowStatus::Failed => ProgressState::Failed(i18n::text(locale, TextKey::ToolFailed).into()),
        RowStatus::Success | RowStatus::Resolved => ProgressState::Complete,
        RowStatus::Cancelled => ProgressState::Cancelled,
        RowStatus::Unknown => ProgressState::Unknown,
    }
}

fn gate_option_label(
    purpose: tact_desktop_core::UiRequestPurpose,
    options: &[Arc<str>],
    index: usize,
    locale: tact_catalog::CatalogLocale,
) -> Option<&str> {
    if matches!(
        purpose,
        tact_desktop_core::UiRequestPurpose::Permission
            | tact_desktop_core::UiRequestPurpose::PlanApproval
    ) {
        match index {
            0 => Some(i18n::text(locale, TextKey::Approved)),
            1 => Some(i18n::text(locale, TextKey::Rejected)),
            2 => Some(i18n::text(locale, TextKey::AlwaysAllow)),
            _ => options.get(index).map(AsRef::as_ref),
        }
    } else {
        options.get(index).map(AsRef::as_ref)
    }
}

fn gate_element(
    gate: &GateCard,
    snapshot: &DesktopSnapshot,
    weak: WeakEntity<DesktopRoot>,
    read_only: bool,
    preview: Option<SharedString>,
    gate_focus: Option<&gpui::FocusHandle>,
) -> AnyElement {
    let locale = snapshot.language;
    let origin = match &gate.origin {
        tact_desktop_core::GateOrigin::Main => i18n::text(locale, TextKey::MainAgent).to_string(),
        tact_desktop_core::GateOrigin::Subagent { child_id, tool_id } => match tool_id {
            Some(tool_id) => format!(
                "{} {child_id} · {} {tool_id}",
                i18n::text(locale, TextKey::SubagentTranscript),
                i18n::text(locale, TextKey::Tool)
            ),
            None => format!(
                "{} {child_id}",
                i18n::text(locale, TextKey::SubagentTranscript)
            ),
        },
        tact_desktop_core::GateOrigin::Tool { tool_id } => {
            format!("{} {tool_id}", i18n::text(locale, TextKey::Tool))
        }
    };
    if gate.status != GateState::Pending {
        let selection = match gate.selection {
            GateSelection::Single(index) => index.and_then(|index| {
                gate_option_label(gate.purpose, &gate.options, index, locale)
                    .map(ToString::to_string)
            }),
            GateSelection::Multiple(ref indexes) => {
                let labels = indexes
                    .iter()
                    .filter_map(|index| {
                        gate_option_label(gate.purpose, &gate.options, *index, locale)
                    })
                    .collect::<Vec<_>>();
                (!labels.is_empty()).then(|| labels.join(", "))
            }
        };
        let settled = if gate.status == GateState::Cancelled {
            Some(i18n::text(locale, TextKey::Cancelled).to_string())
        } else if matches!(
            gate.purpose,
            tact_desktop_core::UiRequestPurpose::Permission
                | tact_desktop_core::UiRequestPurpose::PlanApproval
        ) {
            match gate.selection {
                GateSelection::Single(Some(0)) => {
                    Some(i18n::text(locale, TextKey::Approved).to_string())
                }
                GateSelection::Single(Some(1)) => {
                    Some(i18n::text(locale, TextKey::Rejected).to_string())
                }
                GateSelection::Single(Some(2)) => {
                    Some(i18n::text(locale, TextKey::AlwaysAllow).to_string())
                }
                _ => selection,
            }
        } else {
            selection
        };
        let summary = if gate.status == GateState::Cancelled {
            format!(
                "{} · {}",
                gate_prompt_line(&gate.prompt),
                i18n::text(locale, TextKey::Cancelled)
            )
        } else if let Some(ref settled) = settled {
            format!("{} · {settled}", gate_prompt_line(&gate.prompt))
        } else {
            gate_prompt_line(&gate.prompt).to_string()
        };
        let request_id = gate.request_id.0;
        let (mark, tone) = match gate.status {
            GateState::Resolved => (IconName::CircleCheck, snapshot_theme_success(snapshot)),
            GateState::Submitting => (IconName::LoaderCircle, snapshot_theme_primary(snapshot)),
            GateState::Cancelled => (IconName::CircleX, snapshot_theme_danger(snapshot)),
            _ => (IconName::CircleX, snapshot_row_muted(snapshot)),
        };
        let name = if matches!(
            gate.purpose,
            tact_desktop_core::UiRequestPurpose::Permission
                | tact_desktop_core::UiRequestPurpose::PlanApproval
        ) {
            "approval"
        } else {
            "ask_user"
        };
        // A settled gate keeps the same compact row anatomy as the tools in
        // its group: status glyph, name, summary and the outcome at the edge.
        return h_flex()
            .id(SharedString::from(format!("gate-summary-{request_id}")))
            .debug_selector(move || format!("gate-summary-{request_id}"))
            .role(Role::Status)
            .aria_label(summary.clone())
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(9.))
            .px(px(12.))
            .min_h(px(37.))
            .child(
                div()
                    .flex_none()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(mark).size(px(13.)).text_color(tone)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .font_semibold()
                    .font_family("Lilex")
                    .text_color(snapshot_ink(snapshot))
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(10.))
                    .text_color(snapshot_row_muted(snapshot))
                    .child(gate_prompt_line(&gate.prompt).to_string()),
            )
            .when_some(settled, |row, outcome| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .font_family("Lilex")
                        .text_color(tone)
                        .child(outcome),
                )
            })
            .into_any_element();
    }
    if gate.kind == GateKind::Select
        && matches!(
            gate.purpose,
            tact_desktop_core::UiRequestPurpose::Permission
                | tact_desktop_core::UiRequestPurpose::PlanApproval
        )
    {
        if read_only {
            let request_id = gate.request_id.0;
            return v_flex()
                .id(SharedString::from(format!(
                    "gate-read-only-{}",
                    gate.request_id.0
                )))
                .debug_selector(move || format!("gate-read-only-{request_id}"))
                .role(Role::Status)
                .aria_label(gate.prompt.to_string())
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme::palette(snapshot.theme, snapshot.appearance).border)
                .child(
                    div()
                        .text_sm()
                        .child(gate_prompt_line(&gate.prompt).to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(
                            theme::palette(snapshot.theme, snapshot.appearance).muted_foreground,
                        )
                        .child(origin),
                )
                .child(
                    div()
                        .text_xs()
                        .child(i18n::text(locale, TextKey::ApprovalInMainTranscript)),
                )
                .into_any_element();
        }
        let request_id = gate.request_id.0;
        let session_epoch = gate.session_epoch;
        let kicker = match locale {
            tact_catalog::CatalogLocale::En => format!(
                "{} · {}",
                match gate.purpose {
                    tact_desktop_core::UiRequestPurpose::Permission => "PERMISSION REQUEST",
                    _ => "PLAN APPROVAL",
                },
                origin.to_uppercase()
            ),
            tact_catalog::CatalogLocale::Zh => format!(
                "{} · {}",
                match gate.purpose {
                    tact_desktop_core::UiRequestPurpose::Permission => "权限请求",
                    _ => "计划审批",
                },
                origin
            ),
        };
        // Every host option is a button: the design's card decides in one
        // click, and the runtime can offer four choices (allow once, deny,
        // always for this tool, always for this pattern), so a fixed
        // three-button component would hide one.
        let mut actions = h_flex().flex_wrap().items_center().gap(px(7.));
        for (index, label) in gate.options.iter().enumerate() {
            let weak = weak.clone();
            let option_label = label.to_string();
            let deny = option_label.to_lowercase().contains("deny");
            let hint = if index == 0 {
                Some("↵")
            } else if deny {
                Some("esc")
            } else {
                None
            };
            let mut action = gpui_base::Button::new(("gate-option", index as u64))
                .debug_selector({
                    let selector = format!("gate-option-{index}");
                    move || selector.clone()
                })
                .role(Role::Button)
                .aria_label(option_label.clone())
                .h(px(29.))
                .px(px(10.))
                .rounded(px(5.))
                .text_size(px(10.))
                .focus(|style| {
                    style
                        .border_1()
                        .border_color(snapshot_theme_primary(snapshot))
                })
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(7.))
                        .child(option_label)
                        .when_some(hint, |line, hint| {
                            let hint_color = if index == 0 {
                                theme::palette(snapshot.theme, snapshot.appearance)
                                    .primary_foreground
                            } else if deny {
                                snapshot_theme_danger(snapshot)
                            } else {
                                snapshot_row_muted(snapshot)
                            };
                            line.child(
                                div()
                                    .text_size(px(9.))
                                    .font_family("Lilex")
                                    .text_color(hint_color)
                                    .opacity(0.75)
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(hint_color.opacity(0.4))
                                    .px(px(3.))
                                    .child(hint),
                            )
                        }),
                )
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.dispatch(DesktopIntent::RespondGate {
                            session_epoch,
                            request_id,
                            response: GateResponse::Select(Some(index)),
                        });
                        this.submitted_gates.insert(request_id);
                        // The row collapses under the click; return focus to
                        // the Review trigger when no newer surface owns it.
                        if this.overlay.is_none() {
                            this.restore_focus(window, cx);
                        }
                    });
                });
            if index == 0 {
                action = action
                    .bg(snapshot_theme_primary(snapshot))
                    .text_color(
                        theme::palette(snapshot.theme, snapshot.appearance).primary_foreground,
                    )
                    .font_semibold();
                if let Some(focus) = gate_focus {
                    action = action.track_focus(focus);
                }
            } else if deny {
                action = action.text_color(snapshot_theme_danger(snapshot));
            } else {
                action = action
                    .border_1()
                    .border_color(snapshot_row_border(snapshot))
                    .bg(snapshot_row_surface(snapshot))
                    .text_color(snapshot_ink(snapshot));
            }
            actions = actions.child(action);
        }
        let prompt_command = preview
            .clone()
            .or_else(|| gate_prompt_command(&gate.prompt));
        let command_description = gate_description(locale, gate_tool_name(snapshot, gate));
        let signal = div()
            .flex_none()
            .size(px(34.))
            .rounded_full()
            .border_1()
            .border_color(snapshot_theme_warning(snapshot))
            .text_size(px(20.))
            .font_family("SF Pro Display")
            .text_color(snapshot_theme_warning(snapshot))
            .flex()
            .items_center()
            .justify_center()
            .child("!");
        let copy = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(10.))
                    .font_family("Lilex")
                    .text_color(snapshot_theme_warning(snapshot))
                    .child(kicker),
            )
            .child(
                div()
                    .text_size(px(19.))
                    .font_semibold()
                    .text_color(snapshot_ink(snapshot))
                    .child(gate_prompt_line(&gate.prompt).to_string()),
            )
            .when_some(command_description, |card, description| {
                card.child(
                    div()
                        .text_size(px(11.))
                        .text_color(snapshot_row_muted(snapshot))
                        .child(description),
                )
            })
            .when_some(prompt_command, |card, command| {
                let clamped = command.lines().count() > GATE_COMMAND_PREVIEW_LINES;
                card.child(
                    div()
                        .debug_selector(|| "gate-command-preview".into())
                        .w_full()
                        .min_w_0()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(snapshot_row_border(snapshot))
                        .bg(snapshot_row_background(snapshot))
                        .when(clamped, |block| block.h(px(GATE_COMMAND_MAX_HEIGHT)))
                        .child(
                            div()
                                .w_full()
                                .overflow_y_scrollbar()
                                .px(px(12.))
                                .py(px(8.))
                                .text_size(px(12.))
                                .font_family("Lilex")
                                .text_color(snapshot_ink(snapshot))
                                .child(command),
                        ),
                )
            })
            .child(actions);
        return h_flex()
            .id(SharedString::from(format!("gate-inline-{request_id}")))
            .debug_selector(move || format!("gate-inline-{request_id}"))
            .role(Role::Group)
            .aria_label(gate.prompt.to_string())
            .w_full()
            .items_start()
            .gap(px(13.))
            .rounded(px(12.))
            .border_1()
            .border_color(snapshot_theme_warning(snapshot))
            .bg(snapshot_row_surface(snapshot))
            .p(px(15.))
            .shadow_sm()
            .child(signal)
            .child(copy)
            .into_any_element();
    }
    match gate.kind {
        GateKind::Select => {
            let request_id = gate.request_id.0;
            let session_epoch = gate.session_epoch;
            let weak = weak.clone();
            let selected = match gate.selection {
                GateSelection::Single(index) => index.map(|index| index.to_string()),
                GateSelection::Multiple(ref indexes) => indexes.first().map(ToString::to_string),
            };
            let selected_index = selected
                .as_deref()
                .and_then(|value| value.parse::<usize>().ok());
            let description = match locale {
                tact_catalog::CatalogLocale::En => {
                    "The selected option becomes the answer returned to the tool."
                }
                tact_catalog::CatalogLocale::Zh => "所选选项会作为工具的回答返回。",
            };
            let kicker = match locale {
                tact_catalog::CatalogLocale::En => {
                    format!("ASK_USER · SINGLE SELECT · {}", origin.to_uppercase())
                }
                tact_catalog::CatalogLocale::Zh => {
                    format!("ASK_USER · 单选 · {origin}")
                }
            };
            QuestionFlow::new(
                format!("gate-{}", gate.request_id.0),
                gate.prompt.to_string(),
            )
            .questions([Question::new("answer", description)
                .optional(true)
                .options(gate.options.iter().enumerate().map(|(index, option)| {
                    ChoiceOption::new(index.to_string(), option.to_string())
                }))
                .answered(selected.map(SharedString::from))])
            .question_prefix(match locale {
                tact_catalog::CatalogLocale::En => "Question",
                tact_catalog::CatalogLocale::Zh => "问题",
            })
            .counter_of(match locale {
                tact_catalog::CatalogLocale::En => "of",
                tact_catalog::CatalogLocale::Zh => "/",
            })
            .skip_label(i18n::text(locale, TextKey::Cancel))
            .continue_label(i18n::text(locale, TextKey::Confirm))
            .finish_label(i18n::text(locale, TextKey::Confirm))
            .kicker(kicker)
            .card_options(true)
            .no_selection_label(match locale {
                tact_catalog::CatalogLocale::En => "No selection",
                tact_catalog::CatalogLocale::Zh => "未选择",
            })
            .rounded(px(12.))
            .border_1()
            .border_color(snapshot_row_border(snapshot))
            .bg(snapshot_row_surface(snapshot))
            .p(px(16.))
            .on_event(move |event, window, cx| match event {
                QuestionFlowEvent::Answered { option, .. } => {
                    if let Ok(index) = option.parse::<usize>() {
                        let _ = weak.update(cx, |this, _| {
                            this.dispatch(DesktopIntent::SetGateDraft {
                                session_epoch,
                                request_id,
                                selected: vec![index],
                            });
                        });
                    }
                }
                QuestionFlowEvent::Completed { .. } => {
                    if let Some(index) = selected_index {
                        let _ = weak.update(cx, |this, cx| {
                            this.dispatch(DesktopIntent::RespondGate {
                                session_epoch,
                                request_id,
                                response: GateResponse::Select(Some(index)),
                            });
                            if this.overlay.is_none() {
                                this.restore_focus(window, cx);
                            }
                        });
                    }
                }
                QuestionFlowEvent::Skipped { .. } => {
                    let _ = weak.update(cx, |this, cx| {
                        this.dispatch(DesktopIntent::RespondGate {
                            session_epoch,
                            request_id,
                            response: GateResponse::Select(None),
                        });
                        if this.overlay.is_none() {
                            this.restore_focus(window, cx);
                        }
                    });
                }
                QuestionFlowEvent::Advanced { .. } => {}
            })
            .into_any_element()
        }
        GateKind::MultiSelect => {
            let selected = match &gate.selection {
                GateSelection::Multiple(indexes) => indexes.clone(),
                GateSelection::Single(index) => index.iter().copied().collect(),
            };
            let request_id = gate.request_id.0;
            let session_epoch = gate.session_epoch;
            let initial_selected = selected.clone();
            let live_choices = std::rc::Rc::new(std::cell::RefCell::new(initial_selected.clone()));
            let rows = gate
                .options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let weak = weak.clone();
                    let live_choices = live_choices.clone();
                    let on = selected.contains(&index);
                    let toggle = Toggle::new(format!("gate-option-{}", index), option.to_string())
                        .shape(ToggleShape::Check)
                        .on(on)
                        .on_event(move |event, _, cx| {
                            let ToggleEvent::Toggled { on, .. } = event;
                            let mut choices = live_choices.borrow_mut();
                            if *on {
                                choices.push(index);
                                choices.sort_unstable();
                                choices.dedup();
                            } else {
                                choices.retain(|candidate| *candidate != index);
                            }
                            let next = choices.clone();
                            drop(choices);
                            let _ = weak.update(cx, |this, _| {
                                this.dispatch(DesktopIntent::SetGateDraft {
                                    session_epoch,
                                    request_id,
                                    selected: next,
                                });
                            });
                        });
                    div()
                        .flex_1()
                        .min_w(px(160.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if on {
                            snapshot_theme_primary(snapshot)
                        } else {
                            snapshot_row_border(snapshot)
                        })
                        .bg(snapshot_row_surface(snapshot))
                        .p(px(10.))
                        .child(toggle)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            let confirm_weak = weak;
            let default_choices = initial_selected;
            let kicker = match locale {
                tact_catalog::CatalogLocale::En => {
                    format!("ASK_USER · MULTI SELECT · {}", origin.to_uppercase())
                }
                tact_catalog::CatalogLocale::Zh => format!("ASK_USER · 多选 · {origin}"),
            };
            let description = match locale {
                tact_catalog::CatalogLocale::En => {
                    "Choose any number of checks. The tool receives the selected indices."
                }
                tact_catalog::CatalogLocale::Zh => "可选择任意多项，工具会收到选中的序号。",
            };
            let status = match locale {
                tact_catalog::CatalogLocale::En => format!("{} selected", default_choices.len()),
                tact_catalog::CatalogLocale::Zh => format!("已选 {} 项", default_choices.len()),
            };
            v_flex()
                .id(SharedString::from(format!(
                    "gate-multi-{}",
                    gate.request_id.0
                )))
                .debug_selector({
                    let selector = format!("gate-multi-{}", gate.request_id.0);
                    move || selector.clone()
                })
                .role(Role::Alert)
                .aria_label(gate.prompt.to_string())
                .w_full()
                .gap(px(10.))
                .rounded(px(12.))
                .border_1()
                .border_color(snapshot_row_border(snapshot))
                .bg(snapshot_row_surface(snapshot))
                .p(px(16.))
                .child(
                    div()
                        .text_size(px(10.))
                        .font_family("Lilex")
                        .text_color(snapshot_row_muted(snapshot))
                        .child(kicker),
                )
                .child(
                    div()
                        .text_size(px(18.))
                        .font_semibold()
                        .text_color(snapshot_ink(snapshot))
                        .child(gate.prompt.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(snapshot_row_muted(snapshot))
                        .child(description),
                )
                .child(div().flex().flex_wrap().gap(px(8.)).children(rows))
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(10.))
                                .font_family("Lilex")
                                .text_color(snapshot_row_muted(snapshot))
                                .child(status),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    gpui_base::Button::new(format!(
                                        "gate-multi-cancel-{request_id}"
                                    ))
                                    .role(Role::Button)
                                    .tab_stop(true)
                                    .aria_label(i18n::text(locale, TextKey::Cancel))
                                    .px_2()
                                    .py_1()
                                    .text_size(px(11.))
                                    .text_color(snapshot_row_muted(snapshot))
                                    .child(i18n::text(locale, TextKey::Cancel))
                                    .on_click({
                                        let weak = confirm_weak.clone();
                                        move |_, window, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.dispatch(DesktopIntent::RespondGate {
                                                    session_epoch,
                                                    request_id,
                                                    response: GateResponse::MultiSelect(None),
                                                });
                                                if this.overlay.is_none() {
                                                    this.restore_focus(window, cx);
                                                }
                                            });
                                        }
                                    }),
                                )
                                .child(
                                    gpui_base::Button::new(format!(
                                        "gate-multi-confirm-{request_id}"
                                    ))
                                    .role(Role::Button)
                                    .tab_stop(true)
                                    .aria_label(i18n::text(locale, TextKey::Confirm))
                                    .disabled(default_choices.is_empty())
                                    .rounded(px(5.))
                                    .bg(snapshot_theme_primary(snapshot))
                                    .text_color(
                                        theme::palette(snapshot.theme, snapshot.appearance)
                                            .primary_foreground,
                                    )
                                    .px(px(10.))
                                    .h(px(26.))
                                    .text_size(px(11.))
                                    .child(i18n::text(locale, TextKey::Confirm))
                                    .on_click(
                                        move |_, window, cx| {
                                            let choices = default_choices.clone();
                                            if choices.is_empty() {
                                                return;
                                            }
                                            let _ = confirm_weak.update(cx, |this, cx| {
                                                this.dispatch(DesktopIntent::RespondGate {
                                                    session_epoch,
                                                    request_id,
                                                    response: GateResponse::MultiSelect(Some(
                                                        choices.clone(),
                                                    )),
                                                });
                                                if this.overlay.is_none() {
                                                    this.restore_focus(window, cx);
                                                }
                                            });
                                        },
                                    ),
                                ),
                        ),
                )
                .into_any_element()
        }
    }
}

fn format_elapsed(started: chrono::DateTime<chrono::Utc>) -> String {
    let seconds = chrono::Utc::now()
        .signed_duration_since(started)
        .num_seconds()
        .max(0);
    if seconds >= 3_600 {
        format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn snapshot_row_background(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).muted
}

fn snapshot_row_border(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).border
}

/// Groups hook items by source; a source with any pending item needs review.
fn group_hook_sources(
    trusted: &[tact_desktop_core::HookItemSnapshot],
    pending: &[tact_desktop_core::HookItemSnapshot],
) -> Vec<(String, Vec<tact_desktop_core::HookItemSnapshot>, bool)> {
    let mut sources: Vec<(String, Vec<tact_desktop_core::HookItemSnapshot>, bool)> = Vec::new();
    for (items, is_trusted) in [(pending, false), (trusted, true)] {
        for item in items {
            if let Some(entry) = sources
                .iter_mut()
                .find(|(source, _, _)| source == &item.source)
            {
                entry.1.push(item.clone());
                entry.2 = entry.2 && is_trusted;
            } else {
                sources.push((item.source.clone(), vec![item.clone()], is_trusted));
            }
        }
    }
    sources
}

/// Splits the first prompt into a headline and an optional summary line.
/// The scroller wraps every row in `px_3`; the thread and composer share one
/// 900px column, so the row wrapper must not add its own horizontal inset.
/// Whether the Back to bottom pill should carry the New activity cue.
///
/// The cue belongs to unseen transcript content, not to the reader's
/// position: scrolling up by itself, or a run that has finished, must not
/// claim there is something new.
fn next_new_activity(
    previous: bool,
    scrolled_up: bool,
    tail_following: bool,
    transcript_changed: bool,
    running: bool,
    boundary: bool,
) -> bool {
    if boundary || !running {
        false
    } else if transcript_changed && (scrolled_up || !tail_following) {
        true
    } else {
        previous
    }
}

fn transcript_row_style() -> gpui::StyleRefinement {
    let mut style = gpui::StyleRefinement::default();
    style.padding.left = Some(px(0.).into());
    style.padding.right = Some(px(0.).into());
    // The scroller's default 32px row gap reads as a paragraph break between
    // the activity cards of one turn. The shell owns the tighter rhythm and
    // lets each row's own padding separate prose.
    style.padding.bottom = Some(px(12.).into());
    style
}

/// The rail's quick-filter placeholder, short enough for the 250px rail in
/// both locales.
fn rail_search_placeholder(locale: tact_catalog::CatalogLocale) -> &'static str {
    match locale {
        tact_catalog::CatalogLocale::En => "Find a session",
        tact_catalog::CatalogLocale::Zh => "查找会话",
    }
}

/// The plan card's status pill.
fn plan_status_label(executing: bool, locale: tact_catalog::CatalogLocale) -> &'static str {
    match (locale, executing) {
        (tact_catalog::CatalogLocale::Zh, true) => "执行中",
        (tact_catalog::CatalogLocale::Zh, false) => "已完成",
        (tact_catalog::CatalogLocale::En, true) => "EXECUTING",
        (tact_catalog::CatalogLocale::En, false) => "COMPLETE",
    }
}

/// The plan card's micro-label, with the singular spelled out.
///
/// A one-step plan read "PLAN / 1 STEPS"; the design's label is a count, so
/// the noun has to agree with it.
fn plan_step_label(steps: usize, locale: tact_catalog::CatalogLocale) -> String {
    match locale {
        tact_catalog::CatalogLocale::En => {
            format!("PLAN / {steps} STEP{}", if steps == 1 { "" } else { "S" })
        }
        tact_catalog::CatalogLocale::Zh => format!("计划 / {steps} 步"),
    }
}

fn split_run_text(text: String) -> (String, Option<String>) {
    // Prompts may start with a blank line. The headline is the first non-empty
    // line and the lede is the next one, matching the design's one-line
    // summary; a long multi-paragraph prompt therefore never overflows the
    // hero header.
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(headline) = lines.next() else {
        return (String::new(), None);
    };
    (headline.to_string(), lines.next().map(str::to_string))
}

/// Initials for the local account avatar (for example `rg` -> `RG`).
fn local_user_initials() -> &'static str {
    static INITIALS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INITIALS.get_or_init(|| {
        let name = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        initials_from_name(&name)
    })
}

fn initials_from_name(name: &str) -> String {
    let initials = name
        .chars()
        .filter(|character| character.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if initials.is_empty() {
        "Y".to_string()
    } else {
        initials
    }
}

fn snapshot_row_surface(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).popover
}

/// The design's `--ink`: message labels and the user's avatar fill.
fn snapshot_ink(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).foreground
}

/// The design's `--canvas`: the transcript background, used as the user
/// avatar's ink so the mark reads as a solid stamp in both appearances.
fn snapshot_canvas(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).background
}

/// The run title drops a leading URL, matching the design's headline.
fn strip_leading_url(text: &str) -> String {
    let trimmed = text.trim_start();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"));
    if let Some(rest) = rest
        && let Some((_, tail)) = rest.split_once(char::is_whitespace)
    {
        return tail.trim_start().to_string();
    }
    text.to_string()
}

fn snapshot_theme_primary(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).primary
}

fn snapshot_theme_success(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).success
}

fn snapshot_theme_danger(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).danger
}

fn snapshot_theme_warning(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).warning
}

fn snapshot_row_muted(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).muted_foreground
}

fn snapshot_accent(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    theme::palette(snapshot.theme, snapshot.appearance).primary
}

fn goal_create_arguments(objective: &str, criteria: &str) -> CommandArguments {
    let criteria = criteria
        .split([';', '\n'])
        .map(str::trim)
        .filter(|criterion| !criterion.is_empty())
        .collect::<Vec<_>>();
    let mut tokens = vec!["create".to_string(), objective.trim().to_string()];
    if !criteria.is_empty() {
        tokens.push("--".into());
        tokens.push(criteria.join(";"));
    }
    CommandArguments { tokens }
}

fn overlay_width(window: &Window, desired: f32) -> gpui::Pixels {
    px((f32::from(window.viewport_size().width) - 24.0).clamp(280.0, desired))
}

fn overlay_height(window: &Window, desired: f32) -> gpui::Pixels {
    px((f32::from(window.viewport_size().height) - 24.0).clamp(180.0, desired))
}

fn voice_button_visible(voice: VoiceUi) -> bool {
    voice.enabled && !voice.missing_api_key
}

fn session_title_label(
    locale: tact_desktop_core::CatalogLocale,
    title: &str,
    message_count: i64,
) -> String {
    if !title.trim().is_empty() {
        return title.to_string();
    }
    let key = if message_count == 0 {
        TextKey::NewSession
    } else {
        TextKey::UntitledSession
    };
    i18n::text(locale, key).to_string()
}

fn composer_voice_button(
    voice: VoiceUi,
    label: &'static str,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    let active = voice.listening;
    let color = if active {
        cx.theme().danger
    } else {
        cx.theme().muted_foreground
    };
    gpui_base::Button::new("composer-voice")
        .debug_selector(|| "composer-voice".into())
        .role(Role::Button)
        .tab_stop(true)
        .aria_label(label)
        .cursor_pointer()
        .rounded(px(5.))
        .p(px(4.))
        .when(active, |button| button.bg(cx.theme().danger.opacity(0.14)))
        .hover(|style| style.bg(cx.theme().muted))
        .child(
            Icon::default()
                .path("icons/mic.svg")
                .size(px(14.))
                .text_color(color),
        )
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            if !this.voice.enabled || this.voice.missing_api_key {
                this.show_toast(
                    Toast {
                        title: this.text(TextKey::VoiceInput).into(),
                        body: this.text(TextKey::VoiceInputUnavailable).into(),
                    },
                    false,
                    cx,
                );
                return;
            }
            if this.voice.transcribing {
                return;
            }
            if this.voice.listening {
                this.dispatch(DesktopIntent::Voice(tact_desktop_core::VoiceCommand::Stop));
                this.voice.listening = false;
                this.voice.transcribing = true;
            } else {
                this.dispatch(DesktopIntent::Voice(tact_desktop_core::VoiceCommand::Start));
                this.voice.listening = true;
            }
            cx.notify();
        }))
}

fn composer_insert_button(
    id: &'static str,
    label: &'static str,
    aria_label: &'static str,
    insertion: &'static str,
    cx: &mut Context<DesktopRoot>,
) -> impl IntoElement {
    gpui_base::Button::new(id)
        .role(Role::Button)
        .tab_stop(true)
        .aria_label(aria_label)
        .cursor_pointer()
        .rounded(px(5.))
        .px(px(4.))
        .py(px(2.))
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .hover(|style| style.bg(cx.theme().muted))
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.prompt.update(cx, |prompt, cx| {
                let draft = prompt.draft(cx).to_string();
                prompt.set_draft(format!("{draft}{insertion}"), window, cx);
                prompt.focus(window, cx);
            });
            let draft = this.prompt.read(cx).draft(cx).to_string();
            this.refresh_slash_overlay(&draft, cx);
        }))
}

fn next_inspector(tab: InspectorTab) -> InspectorTab {
    match tab {
        InspectorTab::Run => InspectorTab::Goals,
        InspectorTab::Goals => InspectorTab::Subagents,
        InspectorTab::Subagents => InspectorTab::Context,
        InspectorTab::Context => InspectorTab::Background,
        InspectorTab::Background => InspectorTab::Run,
    }
}

fn previous_inspector(tab: InspectorTab) -> InspectorTab {
    match tab {
        InspectorTab::Run => InspectorTab::Background,
        InspectorTab::Goals => InspectorTab::Run,
        InspectorTab::Subagents => InspectorTab::Goals,
        InspectorTab::Context => InspectorTab::Subagents,
        InspectorTab::Background => InspectorTab::Context,
    }
}

fn find_command(key: &str) -> Option<&'static CommandDescriptor> {
    tact_catalog::builtins()
        .entries()
        .find(|entry| command_key(&entry.id) == key)
}

fn command_key(id: &CommandId) -> String {
    match id {
        CommandId::Builtin(command) => format!("builtin:{}", command.name()),
        CommandId::BuiltinPath { path, .. } => format!("builtin:{}", path.join(":")),
        CommandId::Cli(entry) => format!("cli:{}", entry.as_str()),
        CommandId::Setting(entry) => format!("setting:{}", entry.as_str()),
        CommandId::Composer(entry) => format!("composer:{}", entry.as_str()),
        CommandId::DynamicSkill(name) => format!("skill:{name}"),
    }
}

fn choice_values(id: CommandId, snapshot: &DesktopSnapshot) -> Vec<(String, String)> {
    let locale = snapshot.language;
    let source = find_command(&command_key(&id))
        .and_then(|entry| entry.args.first())
        .and_then(|arg| match arg.value {
            ArgumentValue::Choice(source) => Some(source),
            ArgumentValue::FreeText => None,
        });
    match source {
        Some(ChoiceSource::FilesAndImages) => vec![(
            i18n::text(locale, TextKey::ChooseFile).into(),
            "files".into(),
        )],
        Some(ChoiceSource::Skills) => snapshot
            .extensions
            .snapshot
            .skills
            .iter()
            .map(|skill| (skill.name.clone(), skill.name.clone()))
            .collect(),
        Some(ChoiceSource::Plugins) => snapshot
            .extensions
            .snapshot
            .plugins
            .iter()
            .map(|plugin| (plugin.name.clone(), plugin.name.clone()))
            .collect(),
        Some(ChoiceSource::McpServers) => match &snapshot.surface {
            Some(SurfaceSnapshot::McpServers(servers)) => servers
                .iter()
                .map(|server| (server.name.clone(), server.name.clone()))
                .collect(),
            _ => Vec::new(),
        },
        Some(ChoiceSource::HookSources) => match &snapshot.surface {
            Some(SurfaceSnapshot::Hooks(hooks)) => hooks
                .trusted
                .iter()
                .chain(hooks.pending.iter())
                .map(|hook| (hook.source.clone(), hook.source.clone()))
                .collect(),
            _ => Vec::new(),
        },
        Some(ChoiceSource::Goals) => snapshot
            .inspector
            .goals
            .iter()
            .filter_map(|goal| {
                goal.snapshot
                    .as_ref()
                    .map(|goal| (goal.title.clone(), goal.id.clone()))
            })
            .collect(),
        Some(ChoiceSource::Fixed(set)) => match set {
            tact_catalog::FixedChoiceSet::ReasoningEffort => vec![
                (
                    i18n::text(locale, TextKey::ReasoningLow).into(),
                    "low".into(),
                ),
                (
                    i18n::text(locale, TextKey::ReasoningMedium).into(),
                    "medium".into(),
                ),
                (
                    i18n::text(locale, TextKey::ReasoningHigh).into(),
                    "high".into(),
                ),
                (
                    i18n::text(locale, TextKey::ReasoningExtraHigh).into(),
                    "xhigh".into(),
                ),
                (
                    i18n::text(locale, TextKey::ReasoningMax).into(),
                    "max".into(),
                ),
            ],
            tact_catalog::FixedChoiceSet::PermissionMode => vec![
                (
                    i18n::text(locale, TextKey::PermissionDefault).into(),
                    "default".into(),
                ),
                (
                    i18n::text(locale, TextKey::PermissionPlan).into(),
                    "plan".into(),
                ),
                (
                    i18n::text(locale, TextKey::PermissionAuto).into(),
                    "auto".into(),
                ),
            ],
            tact_catalog::FixedChoiceSet::Theme => vec![
                (i18n::text(locale, TextKey::Light).into(), "light".into()),
                (i18n::text(locale, TextKey::Dark).into(), "dark".into()),
            ],
            tact_catalog::FixedChoiceSet::ThemeFamily => DesktopTheme::ALL
                .iter()
                .map(|theme| (theme.label().into(), theme.value().into()))
                .collect(),
            tact_catalog::FixedChoiceSet::Language => vec![
                (i18n::text(locale, TextKey::English).into(), "en".into()),
                ("中文".into(), "zh".into()),
            ],
            tact_catalog::FixedChoiceSet::Boolean => {
                vec![
                    (i18n::text(locale, TextKey::On).into(), "true".into()),
                    (i18n::text(locale, TextKey::Off).into(), "false".into()),
                ]
            }
        },
        None => Vec::new(),
    }
}

fn setting_choices(id: SettingId, locale: tact_catalog::CatalogLocale) -> Vec<(String, String)> {
    match id {
        SettingId::ReasoningEffort => vec![
            (
                i18n::text(locale, TextKey::ReasoningLow).into(),
                "low".into(),
            ),
            (
                i18n::text(locale, TextKey::ReasoningMedium).into(),
                "medium".into(),
            ),
            (
                i18n::text(locale, TextKey::ReasoningHigh).into(),
                "high".into(),
            ),
            (
                i18n::text(locale, TextKey::ReasoningExtraHigh).into(),
                "xhigh".into(),
            ),
            (
                i18n::text(locale, TextKey::ReasoningMax).into(),
                "max".into(),
            ),
        ],
        SettingId::PermissionMode => vec![
            (
                i18n::text(locale, TextKey::PermissionDefault).into(),
                "default".into(),
            ),
            (
                i18n::text(locale, TextKey::PermissionPlan).into(),
                "plan".into(),
            ),
            (
                i18n::text(locale, TextKey::PermissionAuto).into(),
                "auto".into(),
            ),
        ],
        SettingId::Theme => vec![
            (i18n::text(locale, TextKey::Light).into(), "light".into()),
            (i18n::text(locale, TextKey::Dark).into(), "dark".into()),
        ],
        SettingId::ThemeFamily => DesktopTheme::ALL
            .iter()
            .map(|theme| (theme.label().to_string(), theme.value().to_string()))
            .collect(),
        SettingId::Language => vec![
            (i18n::text(locale, TextKey::English).into(), "en".into()),
            ("中文".into(), "zh".into()),
        ],
        SettingId::Notifications
        | SettingId::HookOutput
        | SettingId::MicroCompact
        | SettingId::TokioConsole
        | SettingId::SkillBodyAutoInject => {
            vec![
                (i18n::text(locale, TextKey::On).into(), "true".into()),
                (i18n::text(locale, TextKey::Off).into(), "false".into()),
            ]
        }
        _ => Vec::new(),
    }
}

fn setting_value(id: SettingId, text: &str) -> FormValue {
    match id {
        SettingId::Notifications
        | SettingId::HookOutput
        | SettingId::MicroCompact
        | SettingId::TokioConsole
        | SettingId::SkillBodyAutoInject => FormValue::Setting(
            id,
            SettingValue::Bool(matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "true" | "on" | "yes" | "1"
            )),
        ),
        SettingId::ReasoningEffort
        | SettingId::PermissionMode
        | SettingId::Theme
        | SettingId::ThemeFamily
        | SettingId::Language => FormValue::Setting(id, SettingValue::Choice(text.trim().into())),
        _ => FormValue::Setting(id, SettingValue::Text(text.trim().into())),
    }
}

fn setting_mode_label(id: SettingId, locale: tact_catalog::CatalogLocale) -> &'static str {
    match tact_desktop_core::setting_mode(id) {
        tact_desktop_core::SettingMode::Live if id == SettingId::SubagentModel => {
            i18n::text(locale, TextKey::NextChild)
        }
        tact_desktop_core::SettingMode::Live => i18n::text(locale, TextKey::Live),
        tact_desktop_core::SettingMode::StartupGlobal => {
            i18n::text(locale, TextKey::RequiresRestart)
        }
    }
}

fn surface_signature(surface: &SurfaceSnapshot) -> String {
    format!("{surface:?}")
}

fn surface_reader_markdown(
    kind: ReaderKind,
    surface: &SurfaceSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> Option<String> {
    match (kind, surface) {
        (ReaderKind::SystemPrompt, SurfaceSnapshot::SystemPrompt(prompt)) => Some(prompt.clone()),
        (ReaderKind::Stats, SurfaceSnapshot::Stats(stats)) => Some(format!(
            "# {}\n\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {} ms",
            i18n::text(locale, TextKey::SessionStatistics),
            i18n::text(locale, TextKey::Turns),
            stats.turns_taken,
            i18n::text(locale, TextKey::Prompt),
            stats.prompt_tokens,
            i18n::text(locale, TextKey::Completion),
            stats.completion_tokens,
            i18n::text(locale, TextKey::Total),
            stats.total_tokens,
            i18n::text(locale, TextKey::Cached),
            stats.cache_hit_tokens,
            i18n::text(locale, TextKey::Reasoning),
            stats.reasoning_tokens,
            i18n::text(locale, TextKey::Elapsed),
            stats.elapsed_ms
        )),
        (ReaderKind::History, SurfaceSnapshot::History(entries)) => {
            let mut markdown = format!("# {}\n\n", i18n::text(locale, TextKey::TaskHistory));
            for entry in entries {
                markdown.push_str(&format!(
                    "- **{}** — {}\n  - {}\n",
                    entry.timestamp, entry.task, entry.summary
                ));
            }
            Some(markdown)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Element as _, Focusable as _, TestAppContext, VisualTestContext, px, size};
    use gpui_component::Root;
    use tact_desktop_core::{
        DesktopConfig, LatestToolView, MessagePhase, SessionEpoch, StateCommit, ToolBody,
        ToolMetaView, ToolOutputView,
    };

    use crate::ui::process::process_row_groupable;

    use super::*;

    fn install_test_config(config: &DesktopConfig) {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| config.install_process_config().unwrap());
    }

    fn install_two_project_sessions(this: &mut DesktopRoot, cx: &mut Context<DesktopRoot>) {
        let first_project = tact_desktop_core::ProjectId("project-a".into());
        let second_project = tact_desktop_core::ProjectId("project-b".into());
        let mut snapshot = (*this.snapshot).clone();
        snapshot.project = tact_desktop_core::ProjectView {
            id: Some(first_project.clone()),
            root: Some("/tmp/project-a".into()),
            display_name: "Project A".into(),
            branch: Some("main".into()),
            repository: None,
            available: true,
        };
        snapshot.projects = vec![
            snapshot.project.clone(),
            tact_desktop_core::ProjectView {
                id: Some(second_project.clone()),
                root: Some("/tmp/project-b".into()),
                display_name: "Project B".into(),
                branch: Some("feature".into()),
                repository: None,
                available: true,
            },
        ]
        .into();
        let now = chrono::Utc::now();
        snapshot.sessions.items = vec![
            tact_desktop_core::SessionSummaryView {
                id: "session-a".into(),
                project_id: first_project,
                root_dir: "/tmp/project-a".into(),
                title: "Session A".into(),
                updated_at: now,
                archived_at: None,
                locked: false,
                message_count: 1,
                search_score: 0,
            },
            tact_desktop_core::SessionSummaryView {
                id: "session-b".into(),
                project_id: second_project,
                root_dir: "/tmp/project-b".into(),
                title: "Session B".into(),
                updated_at: now - chrono::Duration::hours(1),
                archived_at: None,
                locked: false,
                message_count: 1,
                search_score: 0,
            },
        ];
        snapshot.sessions.selected = None;
        this.snapshot = Arc::new(snapshot);
        cx.notify();
    }

    fn set_transcript_session(
        this: &mut DesktopRoot,
        session_id: &str,
        epoch: u64,
        transcript: &TranscriptView,
        cx: &mut Context<DesktopRoot>,
    ) {
        let mut snapshot = (*this.snapshot).clone();
        snapshot.session.id = Some(SessionId(session_id.into()));
        snapshot.session.epoch = SessionEpoch(epoch);
        snapshot.transcript = transcript.clone();
        this.snapshot = Arc::new(snapshot);
        this.pending_row_updates.clear();
        let count = transcript.len();
        this.scroller.update(cx, |state, cx| state.reset(count, cx));
        cx.notify();
    }

    #[test]
    fn voice_button_only_shows_for_a_configured_worker() {
        let voice = |enabled, missing_api_key| VoiceUi {
            enabled,
            missing_api_key,
            listening: false,
            transcribing: false,
        };
        assert!(!voice_button_visible(voice(false, false)));
        assert!(!voice_button_visible(voice(true, true)));
        assert!(voice_button_visible(voice(true, false)));
    }

    #[test]
    fn manual_titles_survive_an_empty_session() {
        use tact_desktop_core::CatalogLocale::{En, Zh};
        assert_eq!(
            session_title_label(En, "Live smoke session", 0),
            "Live smoke session"
        );
        assert_eq!(session_title_label(En, "", 0), "New session");
        assert_eq!(session_title_label(Zh, "", 0), "新会话");
        assert_eq!(session_title_label(En, "", 3), "Untitled session");
    }

    fn probe_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tact-desktop-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn probe_root(
        cx: &mut TestAppContext,
    ) -> (
        Entity<DesktopRoot>,
        &mut VisualTestContext,
        DesktopController,
        PathBuf,
    ) {
        let data_dir = probe_dir("data");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let (root, window) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            DesktopRoot::new_for_test(controller, window, cx)
        });
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        (root, window, shutdown, data_dir)
    }

    fn draw(window: &mut VisualTestContext) {
        window.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Sends one wheel tick over `at` and asserts the first and last rows of
    /// one scroll container both moved by more than a row. The frame is drawn
    /// afterwards, so callers can inspect the post-scroll state.
    fn assert_scrolls_past_cap(
        window: &mut VisualTestContext,
        at: gpui::Point<gpui::Pixels>,
        first: &'static str,
        last: &'static str,
    ) -> gpui::Bounds<gpui::Pixels> {
        let first_before = window.debug_bounds(first).expect(first);
        let last_before = window.debug_bounds(last).expect(last);
        window.simulate_event(gpui::ScrollWheelEvent {
            position: at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-2000.))),
            ..Default::default()
        });
        draw(window);
        let first_after = window.debug_bounds(first).expect(first);
        let last_after = window.debug_bounds(last).expect(last);
        assert!(
            first_before.origin.y - first_after.origin.y > px(100.)
                && last_before.origin.y - last_after.origin.y > px(100.),
            "the list must scroll past its cap: first {first_before:?} -> {first_after:?}, \
             last {last_before:?} -> {last_after:?}"
        );
        last_after
    }

    #[test]
    fn chinese_permission_choices_keep_stable_values() {
        assert_eq!(
            setting_choices(SettingId::PermissionMode, tact_catalog::CatalogLocale::Zh),
            vec![
                (
                    i18n::text(tact_catalog::CatalogLocale::Zh, TextKey::PermissionDefault).into(),
                    "default".into()
                ),
                (
                    i18n::text(tact_catalog::CatalogLocale::Zh, TextKey::PermissionPlan).into(),
                    "plan".into()
                ),
                (
                    i18n::text(tact_catalog::CatalogLocale::Zh, TextKey::PermissionAuto).into(),
                    "auto".into()
                ),
            ]
        );
    }

    #[test]
    fn theme_family_setting_offers_every_desktop_family() {
        assert_eq!(
            setting_choices(SettingId::ThemeFamily, tact_catalog::CatalogLocale::En),
            vec![
                ("Tact".to_string(), "tact".to_string()),
                ("Claude".to_string(), "claude".to_string()),
                ("Cursor".to_string(), "cursor".to_string()),
            ]
        );
        assert!(matches!(
            setting_value(SettingId::ThemeFamily, "claude"),
            FormValue::Setting(SettingId::ThemeFamily, SettingValue::Choice(value))
                if value == "claude"
        ));

        let data_dir = probe_dir("theme-family-value");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        let mut snapshot = DesktopSnapshot::empty(&config);
        snapshot.theme = DesktopTheme::Cursor;
        assert_eq!(
            setting_display_value(SettingId::ThemeFamily, &snapshot),
            "Cursor"
        );
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn accepting_a_slash_command_leaves_the_caret_at_the_end(cx: &mut TestAppContext) {
        let (root, window, _shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.prompt
                    .update(cx, |prompt, cx| prompt.set_draft("/the", window, cx));
                root.refresh_slash_overlay("/the", cx);
                let active = root
                    .slash_overlay
                    .as_ref()
                    .expect("the /theme command is a candidate")
                    .suggestions
                    .iter()
                    .position(|suggestion| suggestion.insertion == "/theme")
                    .expect("theme is in the catalog");
                root.slash_overlay.as_mut().expect("overlay").active = active;

                root.accept_slash_suggestion(false, window, cx);

                assert_eq!(root.prompt.read(cx).draft(cx).as_ref(), "/theme");
                assert_eq!(root.prompt.read(cx).cursor(cx), "/theme".len());
            });
        });
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn cjk_titles_use_the_compact_display_scale() {
        use tact_catalog::CatalogLocale;
        assert!(is_cjk_title(CatalogLocale::Zh, "重构会话存储"));
        assert!(!is_cjk_title(
            CatalogLocale::En,
            "Refactor the session store"
        ));
        assert!(!is_cjk_title(
            CatalogLocale::Zh,
            "Refactor the session store"
        ));
    }

    #[test]
    fn startup_global_settings_display_their_values() {
        let data_dir = probe_dir("settings-values");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        let mut snapshot = DesktopSnapshot::empty(&config);
        snapshot.settings.provider = "deepseek".into();
        snapshot.settings.base_url = "https://api.example.com".into();
        snapshot.settings.api_key_set = true;
        snapshot.settings.max_tokens = Some(8192);
        snapshot.settings.context_window = Some(128_000);
        snapshot.settings.config_path = Some("/tmp/tact.toml".into());
        snapshot.composer.thinking_budget = Some(4_096);
        assert_eq!(
            setting_display_value(SettingId::Provider, &snapshot),
            "deepseek"
        );
        assert_eq!(
            setting_display_value(SettingId::BaseUrl, &snapshot),
            "https://api.example.com"
        );
        assert_eq!(
            setting_display_value(SettingId::ApiKey, &snapshot),
            "••••••"
        );
        assert_eq!(
            setting_display_value(SettingId::MaxTokens, &snapshot),
            "8192"
        );
        assert_eq!(
            setting_display_value(SettingId::ModelContextWindow, &snapshot),
            "128000"
        );
        assert_eq!(
            setting_display_value(SettingId::ConfigPath, &snapshot),
            "/tmp/tact.toml"
        );
        assert_eq!(
            setting_display_value(SettingId::ThinkingBudget, &snapshot),
            "4096"
        );
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn boolean_setting_choices_are_converted_to_bool_values() {
        assert!(matches!(
            setting_value(SettingId::Notifications, "false"),
            FormValue::Setting(SettingId::Notifications, SettingValue::Bool(false))
        ));
    }

    #[gpui::test]
    fn trajectory_page_lays_out_every_record(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let mut snapshot = window.update(|_, cx| (*root.read(cx).snapshot).clone());
        snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.set_snapshot_for_capture(std::sync::Arc::new(snapshot), cx);
                this.capture_set_run_tab(RunTab::Trajectory, cx);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let rows = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            root.read(cx).trajectory_list_rows(&view).len()
        });
        assert!(rows > 0);
        let first = window
            .debug_bounds("trajectory-row-1")
            .expect("a ledger row");
        let record = window
            .debug_bounds("trajectory-record-1")
            .expect("a ledger record");
        assert!(first.size.height >= px(20.), "the row has body height");
        assert!(
            record.origin.x >= first.origin.x && record.origin.y >= first.origin.y,
            "the record sits inside its row"
        );
        assert!(
            record.origin.y + record.size.height <= first.origin.y + first.size.height,
            "the record fits inside its row"
        );
        assert!(window.debug_bounds("trajectory-overview").is_some());
        // The detail column is collapsed until something is selected, so the
        // ledger keeps the full width while the reader scans.
        assert!(window.debug_bounds("trajectory-detail").is_none());

        // Selecting a record opens the detail panel with that record's own
        // input, output and timing — never another record's.
        let (id, input) = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            let record = view
                .records()
                .iter()
                .find(|record| record.input.is_some())
                .expect("a record with an input body")
                .clone();
            (
                record.id,
                record.input.as_deref().unwrap_or_default().to_string(),
            )
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.capture_select_trajectory_record(id, cx));
            window.draw(cx).clear(cx);
        });
        assert!(window.debug_bounds("trajectory-detail").is_some());
        let shown = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            let record = root.read(cx).trajectory_selected_record(&view);
            record.and_then(|record| record.input.as_deref().map(str::to_string))
        });
        assert_eq!(shown.as_deref(), Some(input.as_str()));
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_detail_tabs_switch_between_one_records_facets(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_trajectory(cx);
        // Select a tool record: it has a name, an input, an output and a slot.
        let id = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            view.records()
                .iter()
                .find(|record| {
                    record.tool_name.is_some() && record.input.is_some() && record.output.is_some()
                })
                .expect("a tool record with every facet")
                .id
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.capture_select_trajectory_record(id, cx));
            window.draw(cx).clear(cx);
        });

        // Every tab is mounted and named, so the reader can find them.
        for tab in TrajectoryDetailTab::ALL {
            assert!(
                window.debug_bounds(tab.selector()).is_some(),
                "the {} tab is mounted",
                tab.selector()
            );
        }

        // The panel opens on Summary; the other bodies are not painted yet.
        assert!(window.debug_bounds("trajectory-tab-body-summary").is_some());
        assert!(window.debug_bounds("trajectory-tab-body-payload").is_none());
        assert!(window.debug_bounds("trajectory-tab-body-result").is_none());

        // Switching to Payload mounts that body and unmounts Summary.
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.capture_set_trajectory_tab(TrajectoryDetailTab::Payload, cx)
            });
            window.draw(cx).clear(cx);
        });
        assert!(window.debug_bounds("trajectory-tab-body-payload").is_some());
        assert!(window.debug_bounds("trajectory-tab-body-summary").is_none());

        for tab in [
            TrajectoryDetailTab::Result,
            TrajectoryDetailTab::Schema,
            TrajectoryDetailTab::Timing,
        ] {
            window.update(|window, cx| {
                root.update(cx, |this, cx| this.capture_set_trajectory_tab(tab, cx));
                window.draw(cx).clear(cx);
            });
            let selector = match tab {
                TrajectoryDetailTab::Result => "trajectory-tab-body-result",
                TrajectoryDetailTab::Schema => "trajectory-tab-body-schema",
                TrajectoryDetailTab::Timing => "trajectory-tab-body-timing",
                _ => unreachable!(),
            };
            assert!(
                window.debug_bounds(selector).is_some(),
                "{selector} is mounted"
            );
        }

        // Closing collapses the whole panel.
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.trajectory_selected = None;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(window.debug_bounds("trajectory-detail").is_none());
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_stats_bar_shows_only_figures_the_transcript_can_back(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_trajectory(cx);
        // The demo transcript reports no token usage, so the bar must show the
        // turns/steps count it can back and nothing else — no `0 tok/s`, no
        // `Cache hit 0%`, no `0%` context meter.
        assert!(window.debug_bounds("trajectory-stats").is_some());
        // The bar is built from the catalog, so the counted-noun label must be
        // the lower-case `turns` the reference uses, not the summary bar's
        // capitalised column heading.
        let labels = crate::ui::trajectory::StatsLabels {
            turn: "turn",
            turns: i18n::text(tact_catalog::CatalogLocale::En, TextKey::TurnsUnit),
            step: "step",
            steps: i18n::text(tact_catalog::CatalogLocale::En, TextKey::Steps),
            tokens: i18n::text(tact_catalog::CatalogLocale::En, TextKey::Tokens),
            cache_hit: i18n::text(tact_catalog::CatalogLocale::En, TextKey::CacheHit),
        };
        assert_eq!(
            labels.turns, "turns",
            "the footer counts, it does not label"
        );
        let (summary, line) = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            let summary = view.summary();
            let line = crate::ui::trajectory::StatsLine::new(summary, labels);
            (summary, line)
        });
        assert!(summary.turns > 0, "the fixture has turns");
        assert_eq!(summary.input_tokens, 0, "the fixture reports no usage");
        assert!(line.turns.is_some(), "turns are shown");
        assert_eq!(line.rate, None, "no duration basis, so no rate");
        assert_eq!(line.tokens, None, "no usage, so no token total");
        assert_eq!(line.cache, None, "no cache counters, so no cache share");
        assert_eq!(line.context, None, "no window, so no context meter");

        // With usage reported, every figure appears — so the omissions above
        // are about the missing evidence, not a bar that renders nothing.
        let full = crate::ui::trajectory::StatsLine::new(
            tact_desktop_core::TrajectorySummary {
                turns: 21,
                records: 1_082,
                measured: 1,
                measured_ms: 50_111,
                input_tokens: 338_000_000,
                cache_hit_tokens: 99,
                cache_miss_tokens: 1,
                context_window: Some(200_000),
                context_used: 100_000,
                ..tact_desktop_core::TrajectorySummary::default()
            },
            crate::ui::trajectory::StatsLabels {
                turn: "turn",
                turns: "turns",
                step: "step",
                steps: "steps",
                tokens: "tok",
                cache_hit: "Cache hit",
            },
        );
        assert_eq!(full.turns.as_deref(), Some("21 turns 1082 steps"));
        assert_eq!(full.tokens.as_deref(), Some("338M tok"));
        assert_eq!(full.cache.as_deref(), Some("Cache hit 99%"));
        assert_eq!(full.context.as_deref(), Some("50%"));
        assert!(full.rate.is_some(), "a measured duration gives a rate");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    /// A tool result long enough to overflow the detail column several times
    /// over: 300 numbered lines.
    fn long_output_transcript() -> TranscriptView {
        let mut long = String::new();
        for line in 1..=300 {
            long.push_str(&format!("line {line}: the tool wrote one more line\n"));
        }
        let long: std::sync::Arc<str> = long.into();
        let rows = tact_desktop_core::fixtures::demo_transcript()
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                if let RowBody::ToolGroup(group) = &mut row.body {
                    let items = group
                        .items
                        .iter()
                        .map(|item| match item {
                            ToolGroupItem::Tool(tool) if group.items.first() == Some(item) => {
                                let mut tool = tool.clone();
                                tool.output.detail = Some(long.clone());
                                ToolGroupItem::Tool(tool)
                            }
                            other => other.clone(),
                        })
                        .collect::<Vec<_>>();
                    group.items = items.into();
                }
                std::sync::Arc::new(row)
            })
            .collect::<Vec<_>>();
        TranscriptView::from_rows(rows)
    }

    #[gpui::test]
    fn a_long_tool_result_scrolls_inside_the_detail_column(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let mut snapshot = window.update(|_, cx| (*root.read(cx).snapshot).clone());
        snapshot.transcript = long_output_transcript();
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.set_snapshot_for_capture(std::sync::Arc::new(snapshot), cx);
                this.capture_set_run_tab(RunTab::Trajectory, cx);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });

        // Select the tool record carrying the very long output.
        let id = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            view.records()
                .iter()
                .find(|record| {
                    record
                        .output
                        .as_deref()
                        .is_some_and(|output| output.starts_with("line 1:"))
                })
                .expect("the record with the long output")
                .id
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.capture_select_trajectory_record(id, cx);
                // The result lives behind the Result tab.
                this.capture_set_trajectory_tab(TrajectoryDetailTab::Result, cx);
            });
            window.draw(cx).clear(cx);
        });

        let detail = window
            .debug_bounds("trajectory-detail")
            .expect("the detail panel");
        let body = window
            .debug_bounds("trajectory-detail-body")
            .expect("the detail body");
        let output = window
            .debug_bounds("trajectory-field-Result")
            .expect("the result field");
        assert_eq!(
            detail.size.height,
            window
                .debug_bounds("trajectory-ledger")
                .expect("the ledger column")
                .size
                .height,
            "both columns share the body's height"
        );
        // The fixture really is far longer than the column.
        assert!(
            output.size.height > body.size.height,
            "the output is taller than the column: {} vs {}",
            f32::from(output.size.height),
            f32::from(body.size.height)
        );
        // ...and that content must not paint past the column. This is the
        // regression that matters: an unbounded body let the text draw over
        // the ledger all the way to the window's bottom edge.
        assert!(
            body.size.height <= detail.size.height,
            "the body stays inside the panel: {} vs {}",
            f32::from(body.size.height),
            f32::from(detail.size.height)
        );

        // And the overflow is reachable rather than clipped: the body reports
        // a real scroll range, and setting an offset moves it.
        let max =
            window.update(|_, cx| f32::from(root.read(cx).trajectory_detail_scroll.max_offset().y));
        assert!(
            max > 0.,
            "the overflowing result has somewhere to scroll: {max}"
        );
        window.update(|_, cx| {
            root.read(cx)
                .trajectory_detail_scroll
                .set_offset(gpui::point(px(0.), px(max)));
        });
        let offset =
            window.update(|_, cx| f32::from(root.read(cx).trajectory_detail_scroll.offset().y));
        assert_eq!(
            offset, max,
            "the body scrolls the whole way to the last line"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    /// Boot the shell on the fixed demo transcript with the ledger open.
    fn probe_trajectory(
        cx: &mut TestAppContext,
    ) -> (
        Entity<DesktopRoot>,
        &mut VisualTestContext,
        DesktopController,
        PathBuf,
    ) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let mut snapshot = window.update(|_, cx| (*root.read(cx).snapshot).clone());
        snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.set_snapshot_for_capture(std::sync::Arc::new(snapshot), cx);
                this.capture_set_run_tab(RunTab::Trajectory, cx);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        (root, window, shutdown, data_dir)
    }

    #[gpui::test]
    fn clicking_an_overview_block_selects_the_record_under_it(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_trajectory(cx);
        let bounds = window
            .debug_bounds("trajectory-overview-plot")
            .expect("the overview plot");
        let (origin, width, count) = window.update(|_, cx| {
            let (origin, width) = root.read(cx).trajectory_track.get();
            (origin, width, root.read(cx).trajectory_view().len())
        });
        assert!(width > 0. && count > 0);
        // Click the middle of the fifth column: the overview's own projection
        // decides which record that is, and the page must select exactly it.
        let slot = width / count as f32;
        let x = px(origin + crate::ui::trajectory::LABELS_WIDTH + slot * 4. + slot / 2.);
        let y = px(f32::from(bounds.origin.y) + 4.);
        window.simulate_click(gpui::point(x, y), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let expected = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            view.record(4).map(|record| record.id)
        });
        let selected = window.update(|_, cx| root.read(cx).trajectory_selected);
        assert_eq!(
            selected, expected,
            "the clicked block is the selected record"
        );
        assert!(window.debug_bounds("trajectory-detail").is_some());
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_ledger_search_filters_records_and_the_overview(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_trajectory(cx);
        let all = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            root.read(cx).trajectory_list_rows(&view).len()
        });
        assert!(all > 1);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.capture_set_trajectory_query("cargo fmt", window, cx);
            });
            window.draw(cx).clear(cx);
        });
        let (filtered, view_len) = window.update(|_, cx| {
            let view = root.read(cx).trajectory_view();
            (root.read(cx).trajectory_list_rows(&view).len(), view.len())
        });
        assert!(
            filtered > 0 && filtered < all,
            "the query narrows the ledger"
        );
        // The overview dims rather than drops the records the query excluded,
        // so the run's shape is still visible while searching.
        assert_eq!(
            window.update(|_, cx| root.read(cx).trajectory_view().len()),
            view_len
        );
        // A query with no matches says so instead of showing an empty ledger.
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.capture_set_trajectory_query("zzzz-nothing", window, cx);
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            window.update(|_, cx| {
                let view = root.read(cx).trajectory_view();
                root.read(cx).trajectory_list_rows(&view).len()
            }),
            0
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_run_tabs_switch_back_to_the_chat_page(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_trajectory(cx);
        assert!(window.debug_bounds("trajectory-page").is_some());
        assert!(window.debug_bounds("desktop-transcript").is_none());
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.capture_set_run_tab(RunTab::Chat, cx));
            window.draw(cx).clear(cx);
        });
        assert!(window.debug_bounds("trajectory-page").is_none());
        assert!(window.debug_bounds("desktop-transcript").is_some());
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn overlay_focus_keeps_composer_inert(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt.update(cx, |prompt, cx| {
                    prompt.set_draft("", window, cx);
                    prompt.focus(window, cx);
                });
            });
        });
        window.simulate_input("before");
        let before = window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).to_string());
        assert_eq!(before, "before");

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt.update(cx, |prompt, cx| {
                    prompt.set_draft("", window, cx);
                });
                this.open_model_picker(SettingId::Model, cx);
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_input("leak");
        assert!(window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).is_empty()));

        window.update(|window, cx| {
            root.update(cx, |this, _| {
                this.close_overlay();
                this.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_input("git");
        let (draft, query) = window.update(|_, cx| {
            let root = root.read(cx);
            (
                root.prompt.read(cx).draft(cx).to_string(),
                root.extension_query.clone(),
            )
        });
        assert!(draft.is_empty());
        assert_eq!(query, "git");

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn tab_cycles_inside_an_overlay_focus_trap(cx: &mut TestAppContext) {
        let data_dir = probe_dir("overlay-trap");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let root_slot: std::rc::Rc<std::cell::RefCell<Option<Entity<DesktopRoot>>>> =
            Default::default();
        let slot = root_slot.clone();
        let (_, window) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            let root = cx.new(|cx| DesktopRoot::new_for_test(controller, window, cx));
            *slot.borrow_mut() = Some(root.clone());
            Root::new(root, window, cx)
        });
        let root = root_slot.borrow().clone().expect("desktop root");
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt.update(cx, |prompt, cx| {
                    prompt.set_draft("", window, cx);
                    prompt.focus(window, cx);
                });
                this.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
            });
            window.draw(cx).clear(cx);
        });
        let composer = window.update(|_, cx| root.read(cx).prompt.read(cx).focus_handle(cx));
        for key in ["tab", "tab", "tab", "tab", "tab", "tab", "tab", "shift-tab"] {
            window.simulate_keystrokes(key);
            window.update(|window, cx| {
                window.draw(cx).clear(cx);
                assert!(
                    gpui_base::active_focus_trap(window, cx).is_some(),
                    "{key} escaped the overlay focus trap"
                );
                assert_ne!(
                    window.focused(cx).as_ref(),
                    Some(&composer),
                    "{key} moved focus back to the inert composer"
                );
            });
        }
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn a_plan_yields_to_its_run_group_across_turns() {
        let demo = tact_desktop_core::fixtures::demo_transcript();
        let group = demo
            .iter()
            .find(|row| matches!(&row.body, RowBody::ToolGroup(_)))
            .cloned()
            .expect("tool group");
        let plan = demo
            .iter()
            .find(|row| matches!(&row.body, RowBody::Plan(_)))
            .cloned()
            .expect("plan");
        let user = demo
            .iter()
            .find(|row| matches!(&row.body, RowBody::User(_)))
            .cloned()
            .expect("user turn");
        let mut plan_row = (*plan).clone();
        if let RowBody::Plan(body) = &mut plan_row.body {
            body.steps = Arc::from([tact_desktop_core::PlanStep {
                tool_id: Some(Arc::from("tool-4")),
                label: Arc::from("run the tests"),
                status: RowStatus::Pending,
                detail: Some(Arc::from("shell")),
            }]);
        }
        let RowBody::Plan(plan_body) = &plan_row.body else {
            unreachable!("the row stays a plan");
        };
        let same_run = TranscriptView::from_rows(vec![group.clone(), Arc::new(plan_row.clone())]);
        assert!(
            plan_is_tool_execution(&plan_row, plan_body, &same_run),
            "a later turn's plan belongs to the run's existing group"
        );
        let next_run = TranscriptView::from_rows(vec![group, user, Arc::new(plan_row.clone())]);
        assert!(
            !plan_is_tool_execution(&plan_row, plan_body, &next_run),
            "a group from an earlier run must not hide a new run's plan"
        );
    }

    #[test]
    fn multi_file_review_rows_carry_their_decisions() {
        let files = [
            crate::ui::diff_review::FileDiff {
                path: "a.rs".into(),
                added: 2,
                removed: 1,
            },
            crate::ui::diff_review::FileDiff {
                path: "b.rs".into(),
                added: 0,
                removed: 3,
            },
        ];
        let rows = diff_rows(
            &files,
            &[DiffProposalState::Accepted, DiffProposalState::Pending],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].state_value(), DiffProposalState::Accepted);
        assert_eq!(rows[1].state_value(), DiffProposalState::Pending);
    }

    #[test]
    fn evidence_snippets_are_plain_and_bounded() {
        assert_eq!(
            evidence_snippet("## **Project:** tact\n\nmore"),
            "Project: tact"
        );
        let long = "x".repeat(200);
        assert_eq!(evidence_snippet(&long).chars().count(), 161);
    }

    #[test]
    fn hook_context_preview_keeps_a_title_and_two_plain_lines() {
        let markdown = "# **Auto Memory — session context**\n\n\
                        The fenced block below is reference data.\n\n\
                        ## Recent sessions (5)\n\
                        - Codex checkpoint: tool lifecycle";
        let title = hook_context_title(markdown, "plugin auto-memory-tact");
        let summary = hook_context_summary(markdown, &title);
        assert_eq!(title, "Auto Memory — session context");
        assert!(summary.contains("reference data"));
        assert!(summary.contains("Codex checkpoint"));
        assert!(!summary.contains("**"));
        assert!(!summary.contains("##"));
    }

    #[gpui::test]
    fn plugin_context_card_stays_compact_and_opens_the_reader(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1800.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.session.phase = SessionPhase::Running;
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(1, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let card = window
            .debug_bounds("transcript-row-hook-context-1")
            .expect("plugin context card");
        assert!(
            card.size.height <= px(80.),
            "plugin context should stay compact: {card:?}"
        );
        window.simulate_click(card.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let reader = window.update(|_, cx| {
            let Overlay::Reader(reader) = root.read(cx).overlay.as_ref()? else {
                return None;
            };
            Some((reader.title.to_string(), reader.markdown.to_string()))
        });
        let (title, markdown) = reader.expect("plugin context reader");
        assert!(title.contains("plugin auto-memory-tact"));
        assert!(markdown.contains("No active tasks"));
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn overlays_own_wheel_and_the_transcript_stays_still(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(900.)));
        let rows = (0..40)
            .map(|index| {
                Arc::new(tact_desktop_core::TranscriptRow {
                    id: RowId::new(index),
                    ordinal: index,
                    loop_id: None,
                    time: None,
                    body: RowBody::Notice(tact_desktop_core::transcript::NoticeRow {
                        text: Arc::from(format!("background row {index}")),
                    }),
                })
            })
            .collect::<Vec<_>>();
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = TranscriptView::from_rows(rows);
                snapshot.session.phase = SessionPhase::Running;
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(5, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let background_before = window
            .debug_bounds("transcript-row-notice-5")
            .expect("visible transcript row");
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_overlay(Overlay::Reader(ReaderState::new(
                    "Long reader",
                    (0..160)
                        .map(|line| format!("line {line}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )));
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let overlay = window
            .debug_bounds("reader-overlay")
            .expect("reader overlay");
        let content_before = window
            .debug_bounds("reader-scroll-content")
            .expect("reader content");
        window.simulate_event(gpui::ScrollWheelEvent {
            position: overlay.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-180.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let background_after = window
            .debug_bounds("transcript-row-notice-5")
            .expect("visible transcript row after reader scroll");
        let content_after = window
            .debug_bounds("reader-scroll-content")
            .expect("reader content after scroll");
        assert_eq!(
            background_after.origin.y, background_before.origin.y,
            "the transcript behind the overlay must not scroll"
        );
        assert!(
            content_after.origin.y < content_before.origin.y,
            "the reader body must scroll: before={content_before:?}, after={content_after:?}"
        );
        for overlay in [
            Overlay::CommandCenter,
            Overlay::Settings,
            Overlay::Hooks,
            Overlay::Extensions(ExtensionsTab::Plugins),
            Overlay::SessionManager,
        ] {
            window.update(|window, cx| {
                root.update(cx, |this, cx| {
                    this.overlay = None;
                    this.overlay_stack.clear();
                    this.open_overlay(overlay);
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let transcript_before = window
                .debug_bounds("transcript-row-notice-5")
                .expect("visible transcript row before dialog scroll");
            window.simulate_event(gpui::ScrollWheelEvent {
                position: point(px(640.), px(450.)),
                delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-180.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            window.update(|window, cx| window.draw(cx).clear(cx));
            let transcript_after = window
                .debug_bounds("transcript-row-notice-5")
                .expect("visible transcript row after dialog scroll");
            assert_eq!(
                transcript_after.origin.y, transcript_before.origin.y,
                "the transcript must not move behind an open dialog"
            );
        }
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.overlay = None;
                this.overlay_stack.clear();
                this.open_overlay(Overlay::Settings);
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let settings_list = window
            .debug_bounds("settings-list")
            .expect("settings scroll region");
        let setting_before = window
            .debug_bounds("setting-row-model")
            .expect("first settings row");
        window.simulate_event(gpui::ScrollWheelEvent {
            position: settings_list.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-180.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let setting_after = window
            .debug_bounds("setting-row-model")
            .expect("first settings row after scroll");
        assert!(
            setting_after.origin.y < setting_before.origin.y,
            "non-reader overlay content must scroll: before={setting_before:?}, after={setting_after:?}"
        );
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.overlay = None;
                this.overlay_stack.clear();
                this.refresh_slash_overlay("/", cx);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let slash = window
            .debug_bounds("slash-overlay")
            .expect("slash suggestion popup");
        let transcript_before = window
            .debug_bounds("transcript-row-notice-5")
            .expect("visible transcript row before slash scroll");
        window.simulate_event(gpui::ScrollWheelEvent {
            position: slash.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-180.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let transcript_after = window
            .debug_bounds("transcript-row-notice-5")
            .expect("visible transcript row after slash scroll");
        assert_eq!(
            transcript_after.origin.y, transcript_before.origin.y,
            "the transcript must not scroll behind the slash popup"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn reader_single_click_does_not_select_text(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(900.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_overlay(Overlay::Reader(ReaderState::new(
                    "Selectable reader",
                    "Alpha beta gamma\n\nDelta epsilon",
                )));
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let content = window
            .debug_bounds("selection-actions-surface")
            .expect("reader selection surface");
        let position = point(content.origin.x + px(40.), content.origin.y + px(16.));
        window.simulate_event(gpui::MouseDownEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: gpui::MouseButton::Left,
            click_count: 1,
            first_mouse: true,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let (held_selected, held_has_selection) = window.update(|window, cx| {
            (
                gpui_base::TextSelection::selected_text(window, cx),
                gpui_base::TextSelection::has_selection(window, cx),
            )
        });
        assert!(
            held_selected.is_empty() && !held_has_selection,
            "pressing the mouse without moving must not start a selection: {held_selected:?}"
        );
        window.simulate_event(gpui::MouseUpEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: gpui::MouseButton::Left,
            click_count: 1,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let (selected, has_selection) = window.update(|window, cx| {
            (
                gpui_base::TextSelection::selected_text(window, cx),
                gpui_base::TextSelection::has_selection(window, cx),
            )
        });
        assert!(
            !has_selection,
            "a single click must not leave a selection state"
        );
        assert!(
            selected.is_empty(),
            "a single click must not create a text selection: {selected:?}"
        );
        let reader_selection = window.update(|_, cx| {
            let Overlay::Reader(reader) = root.read(cx).overlay.as_ref()? else {
                return None;
            };
            reader
                .selection
                .as_ref()
                .map(|selection| selection.read(cx).selected_text(cx).to_string())
        });
        assert_eq!(
            reader_selection.as_deref(),
            Some(""),
            "the shared selection surface must also remain empty"
        );
        window.simulate_event(gpui::MouseDownEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: gpui::MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        window.simulate_event(gpui::MouseUpEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: gpui::MouseButton::Left,
            click_count: 2,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let selected = window.update(gpui_base::TextSelection::selected_text);
        let reader_selection = window.update(|_, cx| {
            let Overlay::Reader(reader) = root.read(cx).overlay.as_ref()? else {
                return None;
            };
            reader
                .selection
                .as_ref()
                .map(|selection| selection.read(cx).selected_text(cx).to_string())
        });
        assert!(
            !selected.trim().is_empty()
                || reader_selection
                    .as_ref()
                    .is_some_and(|text| !text.is_empty()),
            "a double click must select the word under the pointer: window={selected:?}, surface={reader_selection:?}"
        );
        let clear_position = position;
        window.simulate_event(gpui::MouseDownEvent {
            position: clear_position,
            modifiers: gpui::Modifiers::default(),
            button: gpui::MouseButton::Left,
            click_count: 1,
            first_mouse: true,
        });
        window.simulate_event(gpui::MouseUpEvent {
            position: clear_position,
            modifiers: gpui::Modifiers::default(),
            button: gpui::MouseButton::Left,
            click_count: 1,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let selected = window.update(gpui_base::TextSelection::selected_text);
        let reader_selection = window.update(|_, cx| {
            let Overlay::Reader(reader) = root.read(cx).overlay.as_ref()? else {
                return None;
            };
            reader
                .selection
                .as_ref()
                .map(|selection| selection.read(cx).selected_text(cx).to_string())
        });
        assert!(
            selected.is_empty() && reader_selection.as_deref() == Some(""),
            "a later single click must clear the selection: window={selected:?}, surface={reader_selection:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn cached_assistant_selection_follows_language_and_appearance(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let (row_id, selection) = window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
            let this = root.read(cx);
            let row_id = this
                .snapshot
                .transcript
                .iter()
                .find(|row| matches!(&row.body, RowBody::Assistant(_)))
                .expect("assistant fixture")
                .id;
            let session_id = this.transcript_session_id();
            let selection = this
                .assistant_selections
                .get(&("main".to_string(), session_id, row_id))
                .expect("cached assistant selection")
                .selection
                .clone();
            (row_id, selection)
        });
        let labels = window.update(|_, cx| selection.read(cx).action_labels());
        assert_eq!(
            labels.iter().map(SharedString::as_ref).collect::<Vec<_>>(),
            ["Ask", "Explain", "Rewrite"]
        );
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.language = tact_catalog::CatalogLocale::Zh;
                snapshot.appearance = Appearance::Dark;
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let (labels, toolbar_label, dark) = window.update(|_, cx| {
            let selection = root
                .read(cx)
                .assistant_selections
                .get(&(
                    "main".to_string(),
                    root.read(cx).transcript_session_id(),
                    row_id,
                ))
                .expect("cached assistant selection")
                .selection
                .read(cx);
            (
                selection.action_labels(),
                selection.toolbar_label_text().clone(),
                selection.text_is_dark(),
            )
        });
        assert_eq!(
            labels.iter().map(SharedString::as_ref).collect::<Vec<_>>(),
            ["询问", "解释", "重写"]
        );
        assert_eq!(
            toolbar_label.as_ref(),
            i18n::text(tact_catalog::CatalogLocale::Zh, TextKey::SelectionActions)
        );
        assert!(dark, "cached selection style follows dark appearance");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn switching_back_to_a_cached_session_reuses_its_ui_state(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let transcript = tact_desktop_core::fixtures::demo_transcript();
        let alpha = SessionId("cache-alpha".into());
        let beta = SessionId("cache-beta".into());
        let row_id = transcript
            .iter()
            .find(|row| matches!(&row.body, RowBody::Assistant(_)))
            .expect("assistant fixture")
            .id;

        let (alpha_view, alpha_selection) = window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "cache-alpha", 70, &transcript, cx);
            });
            window.draw(cx).clear(cx);
            let this = root.read(cx);
            (
                this.transcript_rows
                    .get(&(alpha.clone(), row_id))
                    .expect("alpha row view")
                    .clone(),
                this.assistant_selections
                    .get(&("main".to_string(), alpha.clone(), row_id))
                    .expect("alpha selection")
                    .selection
                    .clone(),
            )
        });

        let beta_view = window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "cache-beta", 71, &transcript, cx);
            });
            window.draw(cx).clear(cx);
            let this = root.read(cx);
            let beta_view = this
                .transcript_rows
                .get(&(beta.clone(), row_id))
                .expect("beta row view")
                .clone();
            assert_ne!(
                beta_view.entity_id(),
                alpha_view.entity_id(),
                "row ids must not be shared across sessions"
            );
            assert!(
                this.transcript_rows.contains_key(&(alpha.clone(), row_id)),
                "switching away must keep the cached session intact"
            );
            beta_view
        });

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "cache-alpha", 72, &transcript, cx);
            });
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            let this = root.read(cx);
            let view = this
                .transcript_rows
                .get(&(alpha.clone(), row_id))
                .expect("alpha row view after return");
            let selection = this
                .assistant_selections
                .get(&("main".to_string(), alpha.clone(), row_id))
                .expect("alpha selection after return");
            assert_eq!(
                view.entity_id(),
                alpha_view.entity_id(),
                "returning must reuse the row view"
            );
            assert_eq!(
                selection.selection.entity_id(),
                alpha_selection.entity_id(),
                "returning must reuse the selection surface"
            );
            assert_eq!(
                view.read(cx).snapshot.session.epoch,
                SessionEpoch(72),
                "the reused view must carry the adopted session epoch"
            );
            assert_eq!(
                this.transcript_rows
                    .get(&(beta.clone(), row_id))
                    .expect("beta row view after return")
                    .entity_id(),
                beta_view.entity_id(),
                "the other warm session's cache must survive"
            );
        });
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn transcript_cache_keeps_six_sessions_and_evicts_all_their_state(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let transcript = tact_desktop_core::fixtures::demo_transcript();
        let row_id = transcript
            .iter()
            .find(|row| matches!(&row.body, RowBody::Assistant(_)))
            .expect("assistant fixture")
            .id;
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "cache-seed", 90, &transcript, cx);
            });
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            root.update(cx, |this, cx| {
                let seed = SessionId("cache-seed".into());
                let row = this
                    .transcript_rows
                    .get(&(seed.clone(), row_id))
                    .expect("seed row")
                    .read(cx)
                    .row
                    .clone();
                let view = this
                    .transcript_rows
                    .get(&(seed.clone(), row_id))
                    .expect("seed row")
                    .clone();
                let selection = this
                    .assistant_selections
                    .get(&("main".to_string(), seed, row_id))
                    .expect("seed selection")
                    .selection
                    .clone();
                this.transcript_rows.clear();
                this.assistant_selections.clear();
                this.expanded_thinking.clear();
                this.transcript_session_cache.clear();
                for index in 0..7 {
                    let session = SessionId(format!("cache-{index}"));
                    this.transcript_rows
                        .insert((session.clone(), row_id), view.clone());
                    this.assistant_selections.insert(
                        ("main".to_string(), session.clone(), row_id),
                        AssistantSelection {
                            row: row.clone(),
                            selection: selection.clone(),
                            language: this.snapshot.language,
                            appearance: this.snapshot.appearance,
                        },
                    );
                    this.expanded_thinking.insert((session.clone(), row_id));
                    this.touch_transcript_session_cache(&session);
                }
                assert_eq!(
                    this.transcript_session_cache.len(),
                    TRANSCRIPT_SESSION_CACHE
                );
                let evicted = SessionId("cache-0".into());
                assert!(
                    !this
                        .transcript_rows
                        .keys()
                        .any(|(session, _)| session == &evicted)
                );
                assert!(
                    !this
                        .assistant_selections
                        .keys()
                        .any(|(_, session, _)| session == &evicted)
                );
                assert!(
                    !this
                        .expanded_thinking
                        .iter()
                        .any(|(session, _)| session == &evicted)
                );
                assert!(
                    this.transcript_rows
                        .contains_key(&(SessionId("cache-1".into()), row_id)),
                    "the next session stays cached"
                );
            });
        });
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn thinking_expands_inline_without_a_reader(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let transcript = tact_desktop_core::fixtures::demo_transcript();
        let alpha = SessionId("thinking-alpha".into());
        let beta = SessionId("thinking-beta".into());
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "thinking-alpha", 80, &transcript, cx);
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(2, cx));
            });
            window.draw(cx).clear(cx);
        });
        let row = window
            .debug_bounds("transcript-row-thinking-2")
            .expect("thinking row");
        // The disclosure toggle sits at the row's leading edge, just inside
        // the Thinking component's header padding.
        let toggle = gpui::point(row.origin.x + px(24.), row.origin.y + px(12.));
        window.simulate_click(toggle, gpui::Modifiers::default());
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            let root = root.read(cx);
            assert!(
                root.expanded_thinking
                    .contains(&(alpha.clone(), RowId::new(2))),
                "the thought expands in place"
            );
            assert!(
                root.overlay.is_none(),
                "expanding a thought must not open a reader"
            );
        });
        let (alpha_view, alpha_row) = window.update(|_, cx| {
            let root = root.read(cx);
            let view = root
                .transcript_rows
                .get(&(alpha.clone(), RowId::new(2)))
                .expect("alpha thinking view")
                .clone();
            (view.entity_id(), view.read(cx).row.clone())
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "thinking-beta", 81, &transcript, cx);
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(2, cx));
            });
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            let root = root.read(cx);
            let beta_view = root
                .transcript_rows
                .get(&(beta, RowId::new(2)))
                .expect("beta thinking view");
            assert!(
                !beta_view.read(cx).thinking_open,
                "one session's disclosure must not expand another session"
            );
            assert!(
                root.transcript_rows
                    .get(&(alpha.clone(), RowId::new(2)))
                    .expect("alpha thinking view after switch")
                    .read(cx)
                    .thinking_open,
                "switching away must preserve the open thought"
            );
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                set_transcript_session(this, "thinking-alpha", 82, &transcript, cx);
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(2, cx));
            });
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            let root = root.read(cx);
            let view = root
                .transcript_rows
                .get(&(alpha.clone(), RowId::new(2)))
                .expect("alpha thinking view after return");
            assert_eq!(
                view.entity_id(),
                alpha_view,
                "the expanded row must be the cached view"
            );
            assert!(
                Arc::ptr_eq(&view.read(cx).row, &alpha_row),
                "returning with unchanged content must not rebuild the row"
            );
            assert!(
                view.read(cx).thinking_open,
                "the session's expansion state must survive the round trip"
            );
        });
        window.simulate_click(toggle, gpui::Modifiers::default());
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            let root = root.read(cx);
            assert!(
                !root.expanded_thinking.contains(&(alpha, RowId::new(2))),
                "a second click collapses the thought"
            );
        });
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn transcript_rows_keep_a_tight_activity_rhythm(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let rows = demo
                    .iter()
                    .filter(|row| matches!(row.ordinal, 0 | 2))
                    .cloned()
                    .collect();
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(rows);
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| state.reset(count, cx));
            });
            window.draw(cx).clear(cx);
        });
        let user = window
            .debug_bounds("transcript-row-user-0")
            .expect("user row");
        let thinking = window
            .debug_bounds("transcript-row-thinking-2")
            .expect("thinking row");
        assert_eq!(
            thinking.origin.y - (user.origin.y + user.size.height),
            px(12.),
            "activity rows sit on the shell's tight rhythm, not the scroller default"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn process_runs_fold_to_one_line_and_expand_to_a_tight_stream(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1600.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let mut rows = demo
                    .iter()
                    .filter(|row| matches!(row.ordinal, 2 | 3))
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(row) = rows.iter_mut().find(|row| row.ordinal == 3) {
                    let mut clean = (**row).clone();
                    if let RowBody::ToolGroup(group) = &mut clean.body {
                        group.items = Arc::from(
                            group
                                .items
                                .iter()
                                .filter(|item| !matches!(item, ToolGroupItem::Gate(_)))
                                .cloned()
                                .collect::<Vec<_>>(),
                        );
                        group.collapsed = true;
                    }
                    *row = Arc::new(clean);
                }
                let tool_id = rows
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => group.items.iter().find_map(|item| {
                            let ToolGroupItem::Tool(tool) = item else {
                                return None;
                            };
                            Some(tool.tool_id.clone())
                        }),
                        _ => None,
                    })
                    .expect("tool id for the transparent plan");
                rows.insert(
                    1,
                    Arc::new(TranscriptRow {
                        id: RowId::new(20),
                        ordinal: 20,
                        loop_id: None,
                        time: None,
                        body: RowBody::Plan(tact_desktop_core::PlanRow {
                            title: Arc::from("execution"),
                            steps: Arc::from([tact_desktop_core::PlanStep {
                                tool_id: Some(tool_id),
                                label: Arc::from("read the chart"),
                                status: RowStatus::Success,
                                detail: None,
                            }]),
                        }),
                    }),
                );
                let mut next_thinking = (**rows
                    .first()
                    .expect("thinking row before the transparent plan"))
                .clone();
                next_thinking.id = RowId::new(21);
                next_thinking.ordinal = 21;
                rows.push(Arc::new(next_thinking));
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(rows);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(0, cx));
            });
            window.draw(cx).clear(cx);
        });
        let summary = window
            .debug_bounds("transcript-process-summary-2")
            .expect("process summary");
        assert!(
            window.debug_bounds("transcript-row-thinking-2").is_none()
                && window.debug_bounds("transcript-row-tool-group-3").is_none(),
            "a collapsed process run must show one summary row, not each child"
        );
        window.simulate_click(summary.center(), gpui::Modifiers::default());
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let thinking = window
            .debug_bounds("transcript-row-thinking-2")
            .expect("thinking row");
        let tools = window
            .debug_bounds("transcript-row-tool-group-3")
            .expect("tool group");
        assert!(
            tools.origin.y - thinking.bottom() <= px(8.),
            "thinking and tool cards in one process stream must not pay the full row gap: \
             thinking={thinking:?}, tools={tools:?}"
        );
        let next_thinking = window
            .debug_bounds("transcript-row-thinking-21")
            .expect("next thinking row");
        assert!(
            next_thinking.origin.y - tools.bottom() <= px(8.),
            "the tool-to-thinking handoff must keep the same stream rhythm: \
             tools={tools:?}, next_thinking={next_thinking:?}"
        );
        let expanded_summary = window
            .debug_bounds("transcript-process-summary-2")
            .expect("expanded process summary");
        window.simulate_click(expanded_summary.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("transcript-row-thinking-2").is_none()
                && window.debug_bounds("transcript-row-tool-group-3").is_none(),
            "collapsing the summary must hide the process children again"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn pending_gate_rows_are_never_hidden_by_a_process_summary(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1200.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let rows = demo
                    .iter()
                    .filter(|row| matches!(row.ordinal, 2 | 3))
                    .map(|row| {
                        let mut row = (**row).clone();
                        if let RowBody::ToolGroup(group) = &mut row.body {
                            let mut items = group.items.to_vec();
                            for item in &mut items {
                                if let ToolGroupItem::Tool(tool) = item {
                                    tool.status = RowStatus::Success;
                                }
                            }
                            group.items = Arc::from(items);
                        }
                        Arc::new(row)
                    })
                    .collect::<Vec<_>>();
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(rows);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(0, cx));
            });
            window.draw(cx).clear(cx);
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window
                .debug_bounds("transcript-process-summary-2")
                .is_some(),
            "the thinking and tool rows share one timed summary"
        );
        let (groups, roles) = window.update(|_, cx| {
            let scroller = root.read(cx).transcript_scroller.read(cx);
            (
                scroller.process_groups.clone(),
                scroller.process_roles.clone(),
            )
        });
        assert_eq!(groups.len(), 1);
        assert!(
            groups[0].active,
            "the pending gate keeps the block's clock running"
        );
        assert_eq!(groups[0].commands, 1);
        assert_eq!(groups[0].files_changed, 1);
        assert_eq!(
            roles[1],
            ProcessRowRole::Normal,
            "the actionable gate row stays outside the fold"
        );
        assert!(
            window.debug_bounds("transcript-row-tool-group-3").is_some(),
            "a pending approval must never be folded behind that summary"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn a_long_tool_group_title_keeps_the_header_one_row(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let mut row = (**demo
                    .iter()
                    .find(|row| row.ordinal == 3)
                    .expect("tool group row"))
                .clone();
                if let tact_desktop_core::RowBody::ToolGroup(group) = &mut row.body {
                    let mut items = group.items.to_vec();
                    if let Some(tact_desktop_core::ToolGroupItem::Tool(tool)) = items.first_mut() {
                        tool.arg_summary = Arc::from(
                            "export https_proxy=http://127.0.0.1:7890\necho \"--- b\"\nbash python3 -c \"\nimport datetime\nprint(1)\n\" ".repeat(3),
                        );
                    }
                    group.items = Arc::from(items);
                }
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript =
                    tact_desktop_core::TranscriptView::from_rows(vec![Arc::new(row)]);
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| state.reset(count, cx));
            });
            window.draw(cx).clear(cx);
        });
        let header = window
            .debug_bounds("tool-group-toggle-tool-group-3")
            .expect("tool group header");
        assert_eq!(
            header.size.height,
            px(37.),
            "a long command must ellipsize, not wrap the compact header"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_reader_message_sits_on_the_right(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(vec![Arc::new(
                    tact_desktop_core::TranscriptRow {
                        id: RowId::new(0),
                        ordinal: 0,
                        loop_id: None,
                        time: None,
                        body: tact_desktop_core::RowBody::User(
                            tact_desktop_core::transcript::UserRow {
                                text: Arc::from("hi"),
                                attachments: Arc::from([]),
                            },
                        ),
                    },
                )]);
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                this.scroller.update(cx, |state, cx| state.reset(1, cx));
            });
            window.draw(cx).clear(cx);
        });
        let row = window
            .debug_bounds("transcript-row-user-0")
            .expect("user row");
        let bubble = window.debug_bounds("user-message-0").expect("user bubble");
        let trailing = (row.origin.x + row.size.width) - (bubble.origin.x + bubble.size.width);
        assert!(
            trailing > px(40.) && trailing < px(80.),
            "the bubble must hug the trailing edge beside the mark, {trailing:?} away"
        );
        assert!(
            bubble.origin.x > row.origin.x + row.size.width / 2.,
            "a short message stays on the right half"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn a_user_message_footer_shows_the_time_and_copies_the_text(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let text = "summarize the migration";
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(vec![Arc::new(
                    tact_desktop_core::TranscriptRow {
                        id: RowId::new(0),
                        ordinal: 0,
                        loop_id: None,
                        time: Some(Arc::from("10:42")),
                        body: tact_desktop_core::RowBody::User(
                            tact_desktop_core::transcript::UserRow {
                                text: Arc::from(text),
                                attachments: Arc::from([]),
                            },
                        ),
                    },
                )]);
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                this.scroller.update(cx, |state, cx| state.reset(1, cx));
            });
            window.draw(cx).clear(cx);
        });
        let time = window
            .debug_bounds("user-time-0")
            .expect("the message time");
        let bubble = window.debug_bounds("user-message-0").expect("user bubble");
        assert!(
            time.origin.y > bubble.origin.y,
            "the time sits below the message, not in its header"
        );
        let copy = window
            .debug_bounds("copy-user-0")
            .expect("the copy control");
        window.simulate_click(copy.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let copied = window
            .update(|_, cx| cx.read_from_clipboard())
            .and_then(|item| match item.entries.into_iter().next() {
                Some(gpui::ClipboardEntry::String(entry)) => Some(entry.text),
                _ => None,
            });
        assert_eq!(
            copied.as_deref(),
            Some(text),
            "the copy control writes the message text"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn a_coalesced_commit_remeasures_the_rows_it_did_not_touch(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let rows = demo
                    .iter()
                    .filter(|row| matches!(row.ordinal, 7 | 3))
                    .cloned()
                    .collect::<Vec<_>>();
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(rows);
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                this.scroller.update(cx, |state, cx| state.reset(2, cx));
            });
            window.draw(cx).clear(cx);
        });
        let short = window
            .debug_bounds("transcript-row-assistant-7")
            .expect("assistant row");

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let mut assistant = (**demo
                    .iter()
                    .find(|row| row.ordinal == 7)
                    .expect("assistant fixture"))
                .clone();
                if let tact_desktop_core::RowBody::Assistant(row) = &mut assistant.body {
                    row.markdown = Arc::from("A paragraph the commit missed.\n\n".repeat(80));
                }
                let tool_group = (**demo
                    .iter()
                    .find(|row| row.ordinal == 3)
                    .expect("tool group fixture"))
                .clone();
                let notice = (**demo
                    .iter()
                    .find(|row| row.ordinal == 9)
                    .expect("notice fixture"))
                .clone();
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(vec![
                    Arc::new(assistant),
                    Arc::new(tool_group),
                    Arc::new(notice),
                ]);
                // The shell only sees the newest commit; the rows it skipped
                // may have changed height too.
                let revision = this.last_commit_revision.saturating_add(3);
                snapshot.revision = revision;
                this.handle_event(
                    DesktopEvent::Committed(StateCommit {
                        revision,
                        snapshot: Arc::new(snapshot),
                        dirty_rows: Arc::from([RowId::new(7)]),
                        sections: SectionMask::TRANSCRIPT,
                    }),
                    cx,
                );
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let grown = window
            .debug_bounds("transcript-row-assistant-7")
            .expect("assistant row");
        assert!(
            grown.size.height > short.size.height + px(200.),
            "a coalesced commit kept a stale row height: {} -> {}",
            short.size.height,
            grown.size.height
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn transcript_readers_mount_their_typed_surfaces(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_reader_transcript();
                this.snapshot = Arc::new(snapshot);
                this.open_transcript_reader_for_test(ReaderRef::ToolOutput(RowId::new(1)), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("reader-ansi").is_some(),
            "ANSI tool output must use the coloured reader"
        );
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_transcript_reader_for_test(ReaderRef::ToolOutput(RowId::new(2)), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("reader-search").is_some(),
            "web-search output must mount SearchResults"
        );
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_transcript_reader_for_test(ReaderRef::Diff(RowId::new(3)), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("reader-diff-table").is_some(),
            "a multi-file patch must mount DiffTable"
        );
        // `scoped_diff_id` prefixes the table-id length (11 for "reader-diff").
        let accept = window
            .debug_bounds("diff-table-accept-11:reader-difffile-0")
            .expect("accept action for the first file");
        window.simulate_click(accept.center(), gpui::Modifiers::default());
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            let decision = root.read(cx).overlay.as_ref().and_then(|overlay| {
                let Overlay::Reader(reader) = overlay else {
                    return None;
                };
                reader.decisions.first().copied()
            });
            assert_eq!(decision, Some(DiffProposalState::Accepted));
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_transcript_reader_for_test(ReaderRef::ToolOutput(RowId::new(5)), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("reader-artifact-panel").is_some(),
            "a written file must mount ArtifactPanel"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn background_readers_preserve_ansi_output(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector.background =
                    Arc::from([tact_desktop_core::BackgroundTaskView {
                        id: Arc::from("ansi-background"),
                        command: Arc::from("echo colored output"),
                        status: tact_protocol::BackgroundTaskStatusSnapshot::Completed,
                        started_at: 0,
                        finished_at: Some(1),
                        output_tail: Arc::from("\u{1b}[31mRED normal\u{1b}[0m"),
                        output_path: None,
                    }]);
                this.snapshot = Arc::new(snapshot);
                this.select_inspector(InspectorTab::Background, cx);
            });
            window.draw(cx).clear(cx);
        });
        let open = window
            .debug_bounds("open-background-ansi-background")
            .expect("the background task open action");
        window.simulate_click(open.center(), gpui::Modifiers::default());
        window.update(|_, cx| {
            assert_eq!(
                root.read(cx).pending_reader,
                Some(ReaderRef::Background("ansi-background".into())),
                "the background task action must preserve its typed reader reference"
            );
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_transcript_reader_for_test(
                    ReaderRef::Background("ansi-background".into()),
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("reader-background-ansi").is_some(),
            "background output with ANSI escapes must use the coloured reader"
        );
        assert!(
            window.debug_bounds("reader-ansi").is_some(),
            "the coloured background surface must mount"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_background_inspector_leads_with_running_work(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector.background = Arc::from([
                    tact_desktop_core::BackgroundTaskView {
                        id: Arc::from("finished"),
                        command: Arc::from("cargo check"),
                        status: tact_desktop_core::BackgroundTaskStatusSnapshot::Completed,
                        started_at: 1,
                        finished_at: Some(2),
                        ..tact_desktop_core::BackgroundTaskView::default()
                    },
                    tact_desktop_core::BackgroundTaskView {
                        id: Arc::from("newer"),
                        command: Arc::from("sleep 30"),
                        status: tact_desktop_core::BackgroundTaskStatusSnapshot::Running,
                        started_at: 900,
                        ..tact_desktop_core::BackgroundTaskView::default()
                    },
                    tact_desktop_core::BackgroundTaskView {
                        id: Arc::from("older"),
                        command: Arc::from("tail -f log"),
                        status: tact_desktop_core::BackgroundTaskStatusSnapshot::Running,
                        started_at: 500,
                        ..tact_desktop_core::BackgroundTaskView::default()
                    },
                ]);
                this.snapshot = Arc::new(snapshot);
                this.select_inspector(InspectorTab::Background, cx);
            });
            window.draw(cx).clear(cx);
        });

        // The tab is reachable as a fifth inspector facet, not a dialog.
        assert!(
            window.debug_bounds("inspector-tab-4").is_some(),
            "Background is the fifth inspector tab"
        );
        assert!(
            window
                .debug_bounds("background-inspector-summary")
                .is_some(),
            "the tab paints its own summary rather than an overlay"
        );

        // Running work leads and the newest start wins within the group; the
        // finished task keeps a row but sinks below both.
        let newer = window
            .debug_bounds("background-row-newer")
            .expect("running task row");
        let older = window
            .debug_bounds("background-row-older")
            .expect("running task row");
        let finished = window
            .debug_bounds("background-row-finished")
            .expect("finished tasks stay listed");
        assert!(
            newer.origin.y < older.origin.y,
            "newest running task leads: {newer:?} vs {older:?}"
        );
        assert!(
            older.origin.y < finished.origin.y,
            "a finished task sinks below running ones: {older:?} vs {finished:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn snapshot_refreshes_workspace_mentions(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let project = probe_dir("project");
        std::fs::write(project.join("alpha.txt"), "probe").unwrap();
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.project.root = Some(project.clone());
                this.snapshot = Arc::new(snapshot);
                this.sync_prompt_state(cx);
                this.prompt
                    .update(cx, |prompt, cx| prompt.focus(window, cx));
            });
        });
        window.simulate_input("@alp");
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(window.debug_bounds("prompt-bar-suggestions").is_some());
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
        let _ = std::fs::remove_dir_all(project);
    }

    #[gpui::test]
    fn goal_commands_open_the_goals_inspector(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.id = Some(SessionId("goal-session".into()));
                this.snapshot = Arc::new(snapshot);
                this.execute_command(
                    CommandId::Builtin(BuiltinCommand::Goal),
                    CommandArguments::default(),
                    window,
                    cx,
                );
                assert!(this.right_visible);
                this.execute_command(
                    CommandId::BuiltinPath {
                        root: BuiltinCommand::Goal,
                        path: vec!["goal", "list"],
                    },
                    CommandArguments::default(),
                    window,
                    cx,
                );
                assert!(this.right_visible);
            });
        });
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn extension_subcommands_open_their_manager(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let cases = [
            (BuiltinCommand::Mcp, ExtensionsTab::Mcp),
            (BuiltinCommand::Plugin, ExtensionsTab::Plugins),
            (BuiltinCommand::Skill, ExtensionsTab::Skills),
        ];
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.project.id = Some(ProjectId("extension-project".into()));
                snapshot.session.id = Some(SessionId("extension-session".into()));
                this.snapshot = Arc::new(snapshot);
                for (root, expected) in cases {
                    this.close_overlay();
                    this.execute_command(
                        CommandId::BuiltinPath {
                            root,
                            path: vec![root.name(), "list"],
                        },
                        CommandArguments::default(),
                        window,
                        cx,
                    );
                    assert!(
                        matches!(
                            this.overlay,
                            Some(Overlay::Extensions(tab)) if tab == expected
                        ),
                        "{root:?} subcommand did not open its manager"
                    );
                }
                this.close_overlay();
                this.execute_command(
                    CommandId::BuiltinPath {
                        root: BuiltinCommand::Hooks,
                        path: vec!["hooks", "list"],
                    },
                    CommandArguments::default(),
                    window,
                    cx,
                );
                assert!(
                    matches!(this.overlay, Some(Overlay::Hooks)),
                    "hooks subcommand did not open the hooks sheet"
                );
            });
        });
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn permission_picker_has_a_keyboard_focus_target(cx: &mut TestAppContext) {
        struct PermissionHarness {
            root: Entity<DesktopRoot>,
        }

        impl Render for PermissionHarness {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.root
                    .update(cx, |root, cx| root.render_permission_picker(cx))
            }
        }

        let data_dir = probe_dir("permission-focus");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let (_, window) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            let root = cx.new(|cx| DesktopRoot::new_for_test(controller, window, cx));
            PermissionHarness { root }
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.update(|window, cx| window.focus_next(cx));
        assert!(window.update(|window, cx| window.focused(cx).is_some()));
        assert!(
            window.debug_bounds("permission-picker-menu").is_none(),
            "the permission choices must start collapsed"
        );
        let trigger = window
            .debug_bounds("permission-picker")
            .expect("permission picker trigger");
        window.simulate_click(trigger.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let menu = window
            .debug_bounds("permission-picker-menu")
            .expect("clicking the permission trigger must open the anchored menu");
        let viewport = window.update(|window, _| window.viewport_size());
        assert!(
            menu.origin.y >= px(0.0) && menu.origin.y + menu.size.height <= viewport.height,
            "permission menu must fit above the composer trigger: menu={menu:?}, viewport={viewport:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn effort_picker_opens_and_takes_a_choice(cx: &mut TestAppContext) {
        struct EffortHarness {
            root: Entity<DesktopRoot>,
        }

        impl Render for EffortHarness {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.root
                    .update(cx, |root, cx| root.render_effort_picker(cx))
            }
        }

        let data_dir = probe_dir("effort-picker");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let (_, window) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            let root = cx.new(|cx| DesktopRoot::new_for_test(controller, window, cx));
            EffortHarness { root }
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("effort-picker-menu").is_none(),
            "the effort choices must start collapsed"
        );
        let trigger = window
            .debug_bounds("effort-picker")
            .expect("effort picker trigger");
        window.simulate_click(trigger.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let menu = window
            .debug_bounds("effort-picker-menu")
            .expect("clicking the effort trigger must open the anchored menu");
        let viewport = window.update(|window, _| window.viewport_size());
        assert!(
            menu.origin.y >= px(0.0) && menu.origin.y + menu.size.height <= viewport.height,
            "effort menu must fit above the composer trigger: menu={menu:?}, viewport={viewport:?}"
        );
        let choice = window
            .debug_bounds("effort-choice-max")
            .expect("the effort menu offers max");
        window.simulate_click(choice.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("effort-picker-menu").is_none(),
            "choosing an effort must close the menu"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn inline_permission_gate_answers_with_the_clicked_option(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let gate = tact_desktop_core::fixtures::demo_gate();
        let request_id = gate.request_id.0;
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.pending_gates = Arc::from([gate.clone()]);
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(count.saturating_sub(1), cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("gate-inline-12").is_some(),
            "a pending permission gate renders inline in the transcript"
        );
        let submitted = window.update(|_, cx| root.read(cx).submitted_gates.contains(&request_id));
        assert!(!submitted, "rendering the gate must not answer it");

        let deny = window.debug_bounds("gate-option-2").expect("deny option");
        window.simulate_click(deny.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let submitted = window.update(|_, cx| root.read(cx).submitted_gates.contains(&request_id));
        assert!(submitted, "clicking an option answers the gate");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn a_long_gate_command_preview_stays_bounded(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let script = (0..60)
            .map(|line| format!("echo line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rows = tact_desktop_core::fixtures::demo_transcript()
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                if let RowBody::ToolGroup(group) = &mut row.body {
                    let mut items = group.items.to_vec();
                    let mut tool_id = None;
                    if let Some(ToolGroupItem::Tool(tool)) = items.first_mut() {
                        tool.arg_summary = Arc::from("bash · long script");
                        tool.arg_full = Some(Arc::from(script.as_str()));
                        tool_id = Some(tool.tool_id.clone());
                    }
                    if let Some(tool_id) = tool_id
                        && let Some(ToolGroupItem::Gate(gate)) = items
                            .iter_mut()
                            .find(|item| matches!(item, ToolGroupItem::Gate(_)))
                    {
                        gate.origin = tact_desktop_core::GateOrigin::Tool { tool_id };
                        gate.prompt = Arc::from("Allow Tact to run this script?");
                    }
                    group.items = Arc::from(items);
                }
                Arc::new(row)
            })
            .collect::<Vec<_>>();
        let transcript = TranscriptView::from_rows(rows);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = transcript;
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(5, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let preview = window
            .debug_bounds("gate-command-preview")
            .expect("the gate repeats the command it asks about");
        assert!(
            preview.size.height <= px(GATE_COMMAND_MAX_HEIGHT),
            "the command preview is bounded so the decision stays in view: {:.1}px",
            preview.size.height.as_f32()
        );
        assert!(
            preview.size.height >= px(100.),
            "the bounded preview still shows the command: {:.1}px",
            preview.size.height.as_f32()
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn a_multiline_gate_prompt_keeps_the_banner_one_line(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let mut gate = tact_desktop_core::fixtures::demo_gate();
        gate.prompt = Arc::from("Run command: python3 - <<'PY'\nimport json\nprint('x')\nPY");
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.pending_gates = Arc::from([gate.clone()]);
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let banner = window.debug_bounds("gate-banner").expect("gate banner");
        assert!(
            banner.size.height <= px(60.),
            "a command prompt stays one line in the banner: {:.1}px",
            banner.size.height.as_f32()
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn gate_review_focuses_the_primary_allow_option(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let gate = tact_desktop_core::fixtures::demo_gate();
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.pending_gates = Arc::from([gate.clone()]);
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let review = window.debug_bounds("review-gate").expect("review button");
        window.simulate_click(review.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        let (focused, primary) = window.update(|window, cx| {
            let root = root.read(cx);
            let row = root
                .snapshot
                .transcript
                .iter()
                .find(|row| matches!(row.body, RowBody::Gate(_)))
                .expect("gate row");
            let session_id = root.transcript_session_id();
            let view = root
                .transcript_rows
                .get(&(session_id, row.id))
                .expect("row view");
            (window.focused(cx), view.read(cx).gate_focus.clone())
        });
        assert_eq!(
            focused.as_ref(),
            Some(&primary),
            "Review must focus the primary allow option without answering the gate"
        );
        let submitted = window.update(|_, cx| root.read(cx).submitted_gates.contains(&12));
        assert!(!submitted, "focusing the allow option must not submit it");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn multi_select_questions_keep_the_toggle_surface(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let mut gate = tact_desktop_core::fixtures::demo_gate();
        gate.purpose = tact_desktop_core::UiRequestPurpose::Question;
        gate.kind = GateKind::MultiSelect;
        gate.selection = GateSelection::Multiple(Vec::new());
        gate.options = Arc::from([Arc::from("first"), Arc::from("second")]);
        let request_id = gate.request_id.0;
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let rows = tact_desktop_core::fixtures::demo_transcript()
                    .iter()
                    .map(|row| {
                        let mut row = (**row).clone();
                        if row.id.value() == request_id {
                            row.body = RowBody::Gate(gate.clone());
                        }
                        Arc::new(row)
                    })
                    .collect::<Vec<_>>();
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::TranscriptView::from_rows(rows);
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(count.saturating_sub(1), cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("gate-multi-12").is_some(),
            "a multi-select question keeps its toggle surface"
        );
        assert!(
            window.debug_bounds("gate-inline-12").is_none(),
            "a multi-select question is not a one-click permission decision"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn goal_create_arguments_preserve_objective_and_acceptance_criteria() {
        let args = goal_create_arguments(
            "  ship desktop  ",
            "Windows builds;\n Linux builds\n\nmacOS builds;",
        );
        assert_eq!(
            args.tokens,
            vec![
                "create",
                "ship desktop",
                "--",
                "Windows builds;Linux builds;macOS builds"
            ]
        );
    }

    #[gpui::test]
    fn inspector_tabs_switch_through_the_real_controller(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.right_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let goals = window
            .debug_bounds("inspector-tab-1")
            .expect("Goals inspector tab");
        window.simulate_click(goals.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            window.update(|_, cx| root.read(cx).snapshot.inspector.tab),
            InspectorTab::Goals
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn read_only_subagent_rows_hide_suggestions_retry_and_gate_actions(cx: &mut TestAppContext) {
        struct ReadOnlyTranscriptProbe {
            root: Entity<DesktopRoot>,
        }

        impl Render for ReadOnlyTranscriptProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let root = self.root.read(cx);
                let snapshot = root.snapshot.clone();
                let weak = self.root.downgrade();
                let transcript = tact_desktop_core::fixtures::demo_transcript();
                let rows = [7_u64, 11, 12]
                    .into_iter()
                    .filter_map(|index| transcript.iter().find(|row| row.ordinal == index).cloned())
                    .collect::<Vec<_>>();
                v_flex().size_full().children(rows.into_iter().map(|row| {
                    transcript_row_element(&row, &snapshot, weak.clone(), None, true, None, false)
                }))
            }
        }

        let data_dir = probe_dir("subagent-read-only");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let (_, window) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            let root = cx.new(|cx| DesktopRoot::new_for_test(controller, window, cx));
            ReadOnlyTranscriptProbe { root }
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(window.debug_bounds("suggestion-0").is_none());
        assert!(window.debug_bounds("retry-error-11").is_none());
        assert!(window.debug_bounds("gate-read-only-12").is_some());
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn overlay_close_restores_trigger_focus(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt
                    .update(cx, |prompt, cx| prompt.focus(window, cx));
            });
            window.draw(cx).clear(cx);
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.remember_focus(window, cx);
                this.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
            });
            window.draw(cx).clear(cx);
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.close_overlay();
                this.restore_focus(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        let (actual, trigger) = window.update(|window, cx| {
            (
                window.focused(cx),
                root.read(cx).prompt.read(cx).focus_handle(cx),
            )
        });
        assert_eq!(
            actual.as_ref(),
            Some(&trigger),
            "closing an overlay must restore the composer trigger focus"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn command_center_fits_a_constrained_window(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(600.), px(600.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.open_command_center(window, cx));
            window.draw(cx).clear(cx);
        });
        let viewport = window.debug_bounds("tact-desktop-root").unwrap();
        let search = window
            .debug_bounds("command-search-tact-command-center")
            .expect("command center search");
        assert!(search.origin.x >= px(0.));
        assert!(search.origin.x + search.size.width <= viewport.size.width);
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn desktop_root_renders_accessible_landmarks_with_nonzero_geometry(cx: &mut TestAppContext) {
        let data_dir = std::env::temp_dir().join(format!(
            "tact-desktop-root-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, events) = DesktopController::start(config).unwrap();
        let controller_for_shutdown = controller.clone();

        let (_, cx) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            drop(events);
            let root = cx.new(|cx| DesktopRoot::new_for_test(controller, window, cx));
            Root::new(root, window, cx)
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        for selector in [
            "tact-desktop-root",
            "desktop-title-bar",
            "command-center-trigger",
            "toggle-inspector",
            "composer-area",
            "permission-picker",
        ] {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("missing rendered landmark {selector}"));
            assert!(bounds.size.width > px(0.), "{selector}");
            assert!(bounds.size.height > px(0.), "{selector}");
        }

        controller_for_shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn session_list_fills_the_rail_instead_of_an_empty_spacer(cx: &mut TestAppContext) {
        use chrono::{TimeZone as _, Utc};
        use tact_desktop_core::{ProjectId, SessionId, SessionSummaryView};

        let (root, window, shutdown, data_dir) = probe_root(cx);
        let base = Utc
            .with_ymd_and_hms(2026, 10, 31, 8, 0, 0)
            .single()
            .unwrap();
        let items = (0..12)
            .map(|index| SessionSummaryView {
                id: format!("session-{index}"),
                project_id: ProjectId("tact".into()),
                root_dir: data_dir.clone(),
                title: format!("Session {index}"),
                updated_at: base - chrono::Duration::minutes(index),
                archived_at: None,
                locked: false,
                message_count: 10 + index,
                search_score: 0,
            })
            .collect();
        window.update(|window, cx| {
            root.update(cx, |root, cx| {
                let mut snapshot = (*root.snapshot).clone();
                snapshot.sessions.items = items;
                snapshot.sessions.selected = Some(SessionId("session-0".into()));
                root.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        let list = window
            .debug_bounds("session-list")
            .expect("session list bounds");
        let view_all = window
            .debug_bounds("view-all-sessions")
            .expect("view all sessions bounds");
        let settings = window
            .debug_bounds("open-settings")
            .expect("rail settings row");
        let list_bottom = list.origin.y + list.size.height;
        assert!(
            view_all.origin.y - list_bottom <= px(12.),
            "session list ends at {list_bottom:?} but 'view all sessions' starts at {:?}; \
             an empty flex spacer is stealing the rail's free height",
            view_all.origin.y
        );
        assert!(
            view_all.origin.y + view_all.size.height <= settings.origin.y + px(12.),
            "view-all row overlaps the Settings entry"
        );
        assert!(
            window.debug_bounds("rail-extensions").is_none(),
            "the extensions block belongs to the Settings surface, not the rail"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_rail_highlights_the_clicked_session_before_the_switch_lands(cx: &mut TestAppContext) {
        use tact_desktop_core::{ProjectId, SessionId, SessionSummaryView};

        let (root, window, shutdown, data_dir) = probe_root(cx);
        let items = ["session-a", "session-b"]
            .into_iter()
            .map(|id| SessionSummaryView {
                id: id.to_string(),
                project_id: ProjectId("tact".into()),
                root_dir: data_dir.clone(),
                title: id.to_string(),
                updated_at: chrono::Utc::now(),
                archived_at: None,
                locked: false,
                message_count: 1,
                search_score: 0,
            })
            .collect();
        window.update(|window, cx| {
            root.update(cx, |root, cx| {
                let mut snapshot = (*root.snapshot).clone();
                snapshot.sessions.items = items;
                // The click lands before the new host reports its own id, so
                // the selection and the open session disagree for one frame.
                snapshot.session.id = Some(SessionId("session-a".into()));
                snapshot.session.phase = SessionPhase::Ready;
                snapshot.sessions.selected = Some(SessionId("session-b".into()));
                root.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        assert!(
            window
                .debug_bounds("sidebar-nav-active-session-b")
                .is_some(),
            "the row the reader clicked must be highlighted immediately"
        );
        assert!(
            window
                .debug_bounds("sidebar-nav-active-session-a")
                .is_none(),
            "the previous session must not keep the rail's active fill"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn permission_gate_summaries_use_localized_decisions() {
        let options: Vec<Arc<str>> =
            vec!["Allow once".into(), "Deny".into(), "Always allow".into()];
        assert_eq!(
            gate_option_label(
                tact_desktop_core::UiRequestPurpose::Permission,
                &options,
                0,
                tact_catalog::CatalogLocale::Zh,
            ),
            Some("已批准")
        );
        assert_eq!(
            gate_option_label(
                tact_desktop_core::UiRequestPurpose::Permission,
                &options,
                1,
                tact_catalog::CatalogLocale::Zh,
            ),
            Some("已拒绝")
        );
        assert_eq!(
            gate_option_label(
                tact_desktop_core::UiRequestPurpose::Permission,
                &options,
                2,
                tact_catalog::CatalogLocale::Zh,
            ),
            Some("始终允许")
        );
    }

    #[gpui::test]
    fn extension_manage_search_filters_visible_rows(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.surface = Some(SurfaceSnapshot::McpServers(vec![
                    tact_protocol::McpServerSnapshot {
                        name: "filesystem".into(),
                        transport: "stdio".into(),
                        source: "test".into(),
                        status: tact_desktop_core::McpServerStatusSnapshot::NotConnected,
                    },
                    tact_protocol::McpServerSnapshot {
                        name: "github".into(),
                        transport: "http".into(),
                        source: "test".into(),
                        status: tact_desktop_core::McpServerStatusSnapshot::NotConnected,
                    },
                ]));
                this.snapshot = Arc::new(snapshot);
                this.extension_query = "filesystem".into();
                this.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
                this.extension_manage = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let count = window.update(|_, cx| {
            root.read(cx)
                .extensions_table
                .read(cx)
                .filters()
                .iter()
                .find(|filter| filter.id() == "mcp")
                .expect("MCP filter")
                .count()
        });
        assert_eq!(count, 1);
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn queued_prompts_start_as_a_count_badge(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.composer.queue = Arc::from([
                    tact_desktop_core::QueuedPromptView {
                        id: 0,
                        text: Arc::from("first queued prompt"),
                        attachments: Arc::from([]),
                    },
                    tact_desktop_core::QueuedPromptView {
                        id: 1,
                        text: Arc::from("second queued prompt"),
                        attachments: Arc::from([]),
                    },
                ]);
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("queue-item-composer-queue-0").is_none(),
            "queued prompts should start collapsed behind the count badge"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn every_catalog_entry_executes_in_the_shell(cx: &mut TestAppContext) {
        // Counting loop iterations proves nothing: `execute_command` returns
        // early on an unavailable entry, so a shell where every command is
        // inert still counted as "executed". Two sessions cover both sides of
        // availability, and each entry must show a real effect on the shell.
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let report = window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let entries = tact_desktop_core::catalog_entries(tact_catalog::builtins())
                    .cloned()
                    .collect::<Vec<_>>();
                assert!(
                    entries.len() >= 80,
                    "catalog is unexpectedly small: {}",
                    entries.len()
                );
                let mut inert = Vec::new();
                let mut unavailable = Vec::new();
                let mut executed = 0;
                for session in ["no-session", "live-session"] {
                    for descriptor in &entries {
                        if matches!(descriptor.id, CommandId::Builtin(BuiltinCommand::Quit)) {
                            continue;
                        }
                        this.close_overlay();
                        this.toast = None;
                        this.recent_commands.clear();
                        let revision_before = this.snapshot.revision;
                        let overlay_before = this.overlay_kind();
                        let inspector_before = (this.snapshot.inspector.tab, this.right_visible);
                        let dispatched_before = this
                            .dispatched_intents
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let prompts_before = this
                            .platform_prompts
                            .load(std::sync::atomic::Ordering::Relaxed);
                        this.execute_command(
                            descriptor.id.clone(),
                            CommandArguments::default(),
                            window,
                            cx,
                        );
                        let toasted = this.toast.is_some();
                        // Any real route either opens a surface, moves the
                        // inspector, changes the snapshot, or sends an intent
                        // to the controller. Recording the command is not an
                        // effect: a visible-but-inert entry would still be
                        // remembered.
                        let moved = this.overlay_kind() != overlay_before
                            || this.snapshot.revision != revision_before
                            || (this.snapshot.inspector.tab, this.right_visible)
                                != inspector_before
                            || this
                                .dispatched_intents
                                .load(std::sync::atomic::Ordering::Relaxed)
                                != dispatched_before
                            || this
                                .platform_prompts
                                .load(std::sync::atomic::Ordering::Relaxed)
                                != prompts_before;
                        if toasted {
                            unavailable.push(format!("{session}:{:?}", descriptor.path));
                        } else if moved {
                            executed += 1;
                        } else {
                            inert.push(format!("{session}:{:?}", descriptor.path));
                        }
                    }
                }
                (executed, inert, unavailable)
            })
        });
        let (executed, inert, unavailable) = report;
        assert!(executed > 0, "no catalog entry produced any shell effect");
        assert!(
            inert.is_empty(),
            "{} catalog entries showed no effect and no unavailable notice: {:?}",
            inert.len(),
            &inert[..inert.len().min(12)]
        );
        eprintln!(
            "executed {executed}, unavailable {} across two sessions",
            unavailable.len()
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn composer_recalls_input_history_with_arrow_keys(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.composer.history =
                    Arc::from([Arc::from("newer prompt"), Arc::from("older prompt")]);
                this.snapshot = Arc::new(snapshot);
                this.prompt.update(cx, |prompt, cx| {
                    prompt.set_draft("", window, cx);
                    prompt.focus(window, cx);
                });
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_keystrokes("up");
        assert_eq!(
            window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).to_string()),
            "newer prompt"
        );
        window.simulate_keystrokes("up");
        assert_eq!(
            window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).to_string()),
            "older prompt"
        );
        window.simulate_keystrokes("down");
        assert_eq!(
            window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).to_string()),
            "newer prompt"
        );
        window.simulate_keystrokes("down");
        assert_eq!(
            window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).to_string()),
            ""
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn tab_traverses_the_shell_controls(cx: &mut TestAppContext) {
        let (_root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus_next(cx);
        });
        let first =
            window.update(|window, cx| window.focused(cx).map(|handle| format!("{handle:?}")));
        window.update(|window, cx| window.focus_next(cx));
        let second =
            window.update(|window, cx| window.focused(cx).map(|handle| format!("{handle:?}")));
        assert!(first.is_some(), "Tab did not focus any shell control");
        assert!(second.is_some(), "a second Tab lost focus");
        assert_ne!(first, second, "Tab stayed on the same control");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn run_inspector_renders_the_latest_tool_chip(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.text_scale = 1.5;
                snapshot.runtime.latest_tool =
                    Some("read_file 工具调用工具调用🙂🙂🙂 README.md".into());
                snapshot.runtime.latest_tool_detail = Some(LatestToolView {
                    tool_id: "call-latest".into(),
                    group: None,
                    row: None,
                    label: "read_file 工具调用工具调用🙂🙂🙂 README.md".into(),
                    duration_ms: Some(120),
                    status: RowStatus::Running,
                });
                this.snapshot = Arc::new(snapshot);
                this.right_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let inspector = window.debug_bounds("run-inspector").expect("run inspector");
        let chip = window
            .debug_bounds("inspector-latest-tool")
            .expect("latest tool chip");
        assert!(chip.size.width > px(0.), "chip has no width");
        assert!(
            chip.origin.x >= inspector.origin.x
                && chip.origin.x + chip.size.width <= inspector.origin.x + inspector.size.width,
            "chip={chip:?} outside inspector={inspector:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn command_cards_render_their_output_not_the_command() {
        let tool = ToolRow {
            id: RowId::new(1),
            tool_id: "bash-1".into(),
            name: "bash".into(),
            arg_summary: "cargo test".into(),
            arg_full: Some("cargo test".into()),
            status: RowStatus::Failed,
            body: ToolBody::None,
            output: ToolOutputView {
                preview: "error: missing".into(),
                detail: Some("error: missing crate".into()),
                truncated: false,
            },
            meta: ToolMetaView::default(),
            subagent: None,
            expanded: true,
            keep_live: false,
            destructive: false,
            duration_ms: None,
        };

        assert_eq!(
            tool_body_markdown(&tool, tact_catalog::CatalogLocale::En),
            "error: missing crate"
        );
    }

    #[test]
    fn truncated_tool_outputs_say_that_only_the_tail_is_retained() {
        let tool = ToolRow {
            id: RowId::new(1),
            tool_id: "bash-1".into(),
            name: "bash".into(),
            arg_summary: "long output".into(),
            arg_full: None,
            status: RowStatus::Success,
            body: ToolBody::None,
            output: ToolOutputView {
                preview: "tail".into(),
                detail: Some("tail".into()),
                truncated: true,
            },
            meta: ToolMetaView::default(),
            subagent: None,
            expanded: true,
            keep_live: false,
            destructive: false,
            duration_ms: None,
        };

        assert_eq!(
            tool_body_markdown(&tool, tact_catalog::CatalogLocale::En),
            "_Only the last 50,000 characters are retained._\n\ntail"
        );
    }

    #[test]
    fn read_image_output_keeps_metadata_and_exposes_the_file_path() {
        let tool = ToolRow {
            id: RowId::new(1),
            tool_id: "image-1".into(),
            name: "read_image".into(),
            arg_summary: "charts/crop_bottom.png".into(),
            arg_full: None,
            status: RowStatus::Success,
            body: ToolBody::None,
            output: ToolOutputView {
                preview: Arc::from(""),
                detail: Some(
                    "<path>/home/rg/Work/charts/crop_bottom.png</path>\n\
                     <type>image</type>\n\
                     <content>\n\
                     image/jpeg image, 1280x354 px, 39578 bytes\n\
                     </content>"
                        .into(),
                ),
                truncated: false,
            },
            meta: ToolMetaView::default(),
            subagent: None,
            expanded: true,
            keep_live: false,
            destructive: false,
            duration_ms: None,
        };

        let envelope = tool.output.detail.as_deref().expect("read_image detail");
        let image = read_image_output(envelope).expect("read_image envelope");
        assert_eq!(
            image.path,
            PathBuf::from("/home/rg/Work/charts/crop_bottom.png")
        );
        assert_eq!(
            image.summary,
            "`/home/rg/Work/charts/crop_bottom.png` · image/jpeg · 1280x354 px · 39578 bytes"
        );
        assert_eq!(
            tool_body_markdown(&tool, tact_catalog::CatalogLocale::En),
            image.summary
        );
        assert!(read_image_output("not an image envelope").is_none());
    }

    #[test]
    fn cancelled_and_unknown_tools_keep_distinct_progress_states() {
        assert_eq!(
            progress_state(RowStatus::Cancelled, tact_catalog::CatalogLocale::En),
            ProgressState::Cancelled
        );
        assert_eq!(
            progress_state(RowStatus::Unknown, tact_catalog::CatalogLocale::Zh),
            ProgressState::Unknown
        );
    }

    #[test]
    fn chip_labels_truncate_by_display_columns() {
        assert_eq!(truncate_display_text("工具调用工具调用", 6), "工具…");
        assert_eq!(truncate_display_text("abcdef", 3), "ab…");
        assert_eq!(truncate_display_text("a\u{301}bcdef", 3), "a\u{301}b…");
        assert_eq!(truncate_display_text("🙂🙂🙂", 5), "🙂🙂…");
        assert_eq!(
            truncate_display_text("read_file README.md", 34),
            "read_file README.md"
        );
        for text in ["工具调用工具调用", "abcdef", "🙂🙂🙂"] {
            for max in 0..=8 {
                assert!(
                    unicode_width::UnicodeWidthStr::width(
                        truncate_display_text(text, max).as_str(),
                    ) <= max,
                    "{text:?} exceeded {max} columns"
                );
            }
        }
    }

    #[gpui::test]
    fn the_latest_tool_chip_closes_the_narrow_inspector(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(900.), px(700.)));
        let mut expected_tool = None;
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let transcript = tact_desktop_core::fixtures::demo_transcript();
                let (group, tool) = transcript
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => {
                            group.items.iter().find_map(|item| match item {
                                ToolGroupItem::Tool(tool) => Some((row.id, tool.id)),
                                _ => None,
                            })
                        }
                        _ => None,
                    })
                    .expect("a tool group");
                expected_tool = Some(tool);
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = transcript;
                snapshot.runtime.latest_tool = Some("read_file README.md".into());
                snapshot.runtime.latest_tool_detail = Some(LatestToolView {
                    tool_id: "call-latest".into(),
                    group: Some(group),
                    row: Some(tool),
                    label: "read_file README.md".into(),
                    duration_ms: Some(120),
                    status: RowStatus::Running,
                });
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| state.reset(count, cx));
                this.right_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let chip = window
            .debug_bounds("inspector-latest-tool")
            .expect("latest tool chip");
        window.simulate_click(chip.center(), gpui::Modifiers::default());
        window.update(|window, cx| {
            root.update(cx, |this, _| {
                assert!(
                    !this.right_visible,
                    "the chip must close the overlay it cannot scroll behind"
                );
                assert_eq!(
                    this.pending_reader,
                    Some(ReaderRef::ToolOutput(
                        expected_tool.expect("expected tool row")
                    )),
                    "the chip must open the exact tool row"
                );
            });
            window.draw(cx).clear(cx);
        });
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn back_to_bottom_floats_above_the_sticky_strip(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(600.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.tail_following = false;
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.session.phase = SessionPhase::Ready;
                snapshot.session.id = Some(SessionId("back-to-bottom-session".into()));
                snapshot.project.id = Some(ProjectId("back-to-bottom-project".into()));
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| state.reset(count, cx));
                // The pill is anchored to the transcript, not the window: with
                // the inspector open it must not float over the inspector.
                this.right_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let tail_following =
            window.update(|_, cx| root.read(cx).transcript_scroller.read(cx).tail_following);
        assert!(!tail_following, "back-to-bottom state reached the scroller");
        assert!(
            window.debug_bounds("back-to-bottom").is_none(),
            "the pill must hide while the transcript is already at the last line"
        );
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(0, cx));
            });
            window.draw(cx).clear(cx);
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.update(|_, cx| root.read(cx).scroller.read(cx).is_scrolled_up()),
            "the fixture must actually scroll away from the tail"
        );
        let sticky = window.debug_bounds("sticky-strip").expect("sticky strip");
        let back = window
            .debug_bounds("back-to-bottom")
            .expect("back-to-bottom control");
        let inspector = window.debug_bounds("run-inspector").expect("run inspector");
        assert!(
            back.origin.y + back.size.height <= sticky.origin.y,
            "back={back:?}, sticky={sticky:?}"
        );
        assert!(
            back.origin.x + back.size.width <= inspector.origin.x,
            "back={back:?} overlaps inspector={inspector:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn extensions_overlays_match_prototype_sizes(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
                this.extension_manage = false;
            });
            window.draw(cx).clear(cx);
        });
        let popover = window
            .debug_bounds("extensions-overlay")
            .expect("extensions popover");
        assert_eq!(popover.size.width, px(520.), "popover width");

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.extension_manage = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let manager = window
            .debug_bounds("extensions-overlay")
            .expect("extensions manager");
        assert_eq!(manager.size.width, px(780.), "manager width");
        assert!(
            (px(320.)..=px(620.)).contains(&manager.size.height),
            "manager height {:?}",
            manager.size.height
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn extensions_popover_is_centered_in_the_window(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|_, cx| cx.set_reduce_motion(true));
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.open_overlay(Overlay::Extensions(ExtensionsTab::Skills));
                this.extension_manage = false;
            });
            window.draw(cx).clear(cx);
        });
        let popover = window
            .debug_bounds("extensions-overlay")
            .expect("extensions popover");
        let viewport = window.update(|window, _| window.viewport_size());
        let expected_x = (f32::from(viewport.width) - f32::from(popover.size.width)) / 2.0;
        let expected_y = (f32::from(viewport.height) - f32::from(popover.size.height)) / 2.0;
        assert!(
            (f32::from(popover.origin.x) - expected_x).abs() <= 1.0,
            "popover x {} must center at {expected_x}",
            f32::from(popover.origin.x)
        );
        assert!(
            (f32::from(popover.origin.y) - expected_y).abs() <= 1.0,
            "popover y {} must center at {expected_y}",
            f32::from(popover.origin.y)
        );

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.extension_manage = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let manager = window
            .debug_bounds("extensions-overlay")
            .expect("extensions manager");
        let expected_x = (f32::from(viewport.width) - f32::from(manager.size.width)) / 2.0;
        let expected_y = (f32::from(viewport.height) - f32::from(manager.size.height)) / 2.0;
        assert!(
            (f32::from(manager.origin.x) - expected_x).abs() <= 1.0,
            "manager x {} must center at {expected_x}",
            f32::from(manager.origin.x)
        );
        assert!(
            (f32::from(manager.origin.y) - expected_y).abs() <= 1.0,
            "manager y {} must center at {expected_y}",
            f32::from(manager.origin.y)
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn pressing_enter_on_a_searched_skill_inserts_it_into_the_composer(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |root, cx| {
                let mut snapshot = (*root.snapshot).clone();
                snapshot.extensions.snapshot.skills = vec![tact_host::SkillSnapshot {
                    name: "rust-skills".into(),
                    description: "Comprehensive Rust coding guidelines".into(),
                    enabled: true,
                    path: "/skills/rust-skills/SKILL.md".into(),
                }];
                root.snapshot = Arc::new(snapshot);
                root.open_overlay(Overlay::Extensions(ExtensionsTab::Skills));
                root.extension_manage = false;
                root.extension_query = "rust".into();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        window.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.extension_search.update(cx, |input, cx| {
                    input.set_value("rust", window, cx);
                    input.focus(window, cx);
                });
            });
        });

        window.simulate_keystrokes("enter");

        let (draft, overlay) = window.update(|_, cx| {
            let root = root.read(cx);
            (
                root.prompt.read(cx).draft(cx).to_string(),
                root.overlay.is_some(),
            )
        });
        assert_eq!(draft, "/skill rust-skills ");
        assert!(!overlay, "inserting a skill closes the extensions popover");

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn composer_shows_stop_and_send_while_running(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                this.snapshot = Arc::new(snapshot);
                this.prompt.update(cx, |prompt, cx| {
                    prompt.set_progress(ProgressState::Running, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("prompt-bar-stop-control").is_some(),
            "stop control"
        );
        assert!(
            window.debug_bounds("prompt-bar-send-control").is_some(),
            "send control"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn composer_and_transcript_share_one_column_with_the_inspector(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1600.), px(900.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                this.snapshot = Arc::new(snapshot);
                this.right_visible = true;
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(0, cx);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let row = window
            .debug_bounds("transcript-row-user-0")
            .expect("user row");
        let composer = window
            .debug_bounds("composer-frame")
            .expect("composer frame");
        assert!(
            (row.origin.x - composer.origin.x).abs() <= px(1.),
            "row x {} vs composer x {}",
            row.origin.x,
            composer.origin.x
        );
        assert!(
            (row.size.width - composer.size.width).abs() <= px(1.),
            "row width {} vs composer width {}",
            row.size.width,
            composer.size.width
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn escape_denies_the_pending_gate(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.pending_gates = Arc::from([tact_desktop_core::fixtures::demo_gate()]);
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.dismiss_topmost(window, cx));
        });
        let (overlay_closed, submitted) = window.update(|_, cx| {
            let root = root.read(cx);
            (root.overlay.is_none(), root.submitted_gates.contains(&12))
        });
        assert!(
            overlay_closed,
            "a pending gate no longer opens a dialog to close"
        );
        assert!(submitted, "escape must deny the pending gate");

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn escape_cancels_a_run_before_closing_the_rail(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                this.snapshot = Arc::new(snapshot);
                this.left_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.dismiss_topmost(window, cx));
        });
        let left_visible = window.update(|_, cx| root.read(cx).left_visible);
        assert!(
            left_visible,
            "escape must interrupt the run instead of hiding the rail"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn overlays_match_prototype_sizes(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_command_center(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        let palette = window
            .debug_bounds("command-center-overlay")
            .expect("command center");
        assert_eq!(palette.size.width, px(700.), "palette width");

        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.close_overlay();
                this.open_overlay(Overlay::SessionManager);
            });
            window.draw(cx).clear(cx);
        });
        let manager = window
            .debug_bounds("session-manager-overlay")
            .expect("session manager");
        assert_eq!(manager.size.width, px(1040.), "session manager width");
        assert_eq!(manager.size.height, px(720.), "session manager height");

        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.close_overlay();
                this.open_overlay(Overlay::Subagent("child-2".to_owned()));
            });
            window.draw(cx).clear(cx);
        });
        let drawer = window
            .debug_bounds("subagent-overlay")
            .expect("subagent drawer");
        assert_eq!(drawer.size.width, px(520.), "subagent drawer width");

        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.close_overlay();
                this.open_overlay(Overlay::Hooks);
            });
            window.draw(cx).clear(cx);
        });
        let hooks = window.debug_bounds("hooks-overlay").expect("hooks sheet");
        assert_eq!(hooks.size.width, px(780.), "hooks sheet width");
        assert!(
            (px(320.)..=px(620.)).contains(&hooks.size.height),
            "hooks height {:?}",
            hooks.size.height
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn session_manager_takes_focus_away_from_the_composer(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt
                    .update(cx, |prompt, cx| prompt.focus(window, cx));
                this.open_overlay(Overlay::SessionManager);
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let (focused, composer, search) = window.update(|window, cx| {
            let root = root.read(cx);
            (
                window.focused(cx),
                root.prompt.read(cx).focus_handle(cx),
                root.session_search.read(cx).focus_handle(cx),
            )
        });
        assert_ne!(
            focused.as_ref(),
            Some(&composer),
            "the chat composer must not keep focus behind the Session Manager"
        );
        assert_eq!(
            focused.as_ref(),
            Some(&search),
            "the manager's search field owns focus while it is open"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn closing_session_manager_discards_its_search_query(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.open_overlay(Overlay::SessionManager);
                this.session_search.update(cx, |input, cx| {
                    input.set_value("spawn", window, cx);
                });
                this.close_overlay();
                assert!(
                    this.reset_session_search_on_open,
                    "closing the manager must schedule the visible query to clear"
                );
            });
        });

        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.open_overlay(Overlay::SessionManager);
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });

        window.update(|_, cx| {
            let root = root.read(cx);
            assert_eq!(
                root.session_search.read(cx).value().as_ref(),
                "",
                "the next manager opening must start with the stale query cleared"
            );
            assert!(!root.reset_session_search_on_open);
        });

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn global_shortcuts_do_not_act_behind_an_overlay(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.left_visible = false;
                this.prompt
                    .update(cx, |prompt, cx| prompt.focus(window, cx));
                this.open_overlay(Overlay::SessionManager);
            });
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        window.simulate_keystrokes("secondary-b");
        let left_visible = window.update(|_, cx| root.read(cx).left_visible);
        assert!(
            !left_visible,
            "the rail toggle must not act on the page behind the manager"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn voice_transcript_lands_in_the_composer(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt.update(cx, |prompt, cx| {
                    prompt.set_draft("", window, cx);
                    prompt.focus(window, cx);
                });
                this.voice.enabled = true;
                this.handle_event(
                    DesktopEvent::Voice(tact_desktop_core::VoiceEvent::RecordingStarted),
                    cx,
                );
                assert!(this.voice.listening);
                this.handle_event(
                    DesktopEvent::Voice(tact_desktop_core::VoiceEvent::Transcript(
                        "hello from voice".into(),
                    )),
                    cx,
                );
                assert!(!this.voice.listening);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let draft = window.update(|_, cx| root.read(cx).prompt.read(cx).draft(cx).to_string());
        assert_eq!(draft, "hello from voice");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn session_switch_keeps_composer_focus(cx: &mut TestAppContext) {
        // A session switch arrives as one `Committed` event whose snapshot has a
        // new epoch and a Ready phase. The composer must end up focused, which
        // is what lets the user type into the new session immediately. Asserting
        // a queue index under this name passes without ever switching a session.
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut running = (*this.snapshot).clone();
                running.session.phase = SessionPhase::Running;
                running.session.epoch = SessionEpoch(1);
                running.session.id = Some(SessionId("old-session".into()));
                this.snapshot = Arc::new(running);

                let mut switched = (*this.snapshot).clone();
                switched.session.phase = SessionPhase::Ready;
                switched.session.epoch = SessionEpoch(2);
                switched.session.id = Some(SessionId("new-session".into()));
                switched.revision = this.snapshot.revision.saturating_add(1);
                this.handle_event(
                    DesktopEvent::Committed(StateCommit {
                        revision: switched.revision,
                        snapshot: Arc::new(switched),
                        dirty_rows: Arc::from([]),
                        sections: SectionMask::SESSION,
                    }),
                    cx,
                );
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let (phase, focused, composer) = window.update(|window, cx| {
            let (phase, composer) = {
                let this = root.read(cx);
                (
                    this.snapshot.session.phase.clone(),
                    this.prompt.read(cx).focus_handle(cx),
                )
            };
            (phase, window.focused(cx), composer)
        });
        assert_eq!(phase, SessionPhase::Ready);
        assert_eq!(
            focused.as_ref(),
            Some(&composer),
            "a session switch must leave the composer focused for the new session"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn empty_tasks_reader_explains_there_are_no_tasks(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, _cx| {
                this.open_typed_reader(ReaderKind::TasksDag, "Tasks");
                this.refresh_typed_reader();
            });
            window.draw(cx).clear(cx);
        });
        let markdown = window.update(|_, cx| match &root.read(cx).overlay {
            Some(Overlay::Reader(reader)) => reader.markdown.to_string(),
            _ => String::new(),
        });
        assert!(
            markdown.contains("No tasks"),
            "empty tasks reader showed {markdown:?}"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn hook_sources_with_pending_items_stay_untrusted() {
        use tact_desktop_core::HookItemSnapshot;
        let item = |source: &str, command: &str| HookItemSnapshot {
            source: source.to_string(),
            event: "post_tool".into(),
            matcher: None,
            command: command.to_string(),
        };
        let trusted = [item("a", "fmt")];
        let pending = [item("a", "check"), item("b", "notify")];
        let sources = group_hook_sources(&trusted, &pending);
        let a = sources
            .iter()
            .find(|(source, _, _)| source == "a")
            .expect("source a");
        assert!(!a.2, "a source with a pending hook must stay untrusted");
        let b = sources
            .iter()
            .find(|(source, _, _)| source == "b")
            .expect("source b");
        assert!(!b.2);
    }

    #[test]
    fn run_titles_split_a_second_line_into_the_summary() {
        let (title, subtitle) = split_run_text("分析这个库\n以第2个目标来设计".into());
        assert_eq!(title, "分析这个库");
        assert_eq!(subtitle.as_deref(), Some("以第2个目标来设计"));
        let (title, subtitle) = split_run_text("single line".into());
        assert_eq!(title, "single line");
        assert_eq!(subtitle, None);
        let (title, subtitle) = split_run_text("\n  正文".into());
        assert_eq!(title, "正文");
        assert_eq!(subtitle, None);
        let (title, _) = split_run_text("headline\n  ".into());
        assert_eq!(title, "headline");
        let (title, subtitle) = split_run_text("\n\n分析这个库\n以第2个目标来设计\n".into());
        assert_eq!(title, "分析这个库");
        assert_eq!(subtitle.as_deref(), Some("以第2个目标来设计"));
        let (title, subtitle) = split_run_text("headline\nbody one\nbody two".into());
        assert_eq!(title, "headline");
        assert_eq!(subtitle.as_deref(), Some("body one"));
        let (title, subtitle) = split_run_text("\nheadline\n\nbody".into());
        assert_eq!(title, "headline");
        assert_eq!(subtitle.as_deref(), Some("body"));
        let (title, subtitle) = split_run_text(" \n\t \n".into());
        assert_eq!(title, "");
        assert_eq!(subtitle, None);
    }

    #[test]
    fn only_avatar_rows_start_at_the_transcript_edge() {
        let transcript = tact_desktop_core::fixtures::demo_transcript();
        let find = |predicate: fn(&RowBody) -> bool| {
            transcript
                .iter()
                .find(|row| predicate(&row.body))
                .expect("fixture row")
        };
        let user = find(|body| matches!(body, RowBody::User(_)));
        let assistant = find(|body| matches!(body, RowBody::Assistant(_)));
        let plan = find(|body| matches!(body, RowBody::Plan(_)));
        assert!(!row_starts_at_message_column(&user.body));
        assert!(!row_starts_at_message_column(&assistant.body));
        assert!(
            row_starts_at_message_column(&plan.body),
            "a plan card aligns with the message column, not the transcript edge"
        );
    }

    #[test]
    fn plan_step_label_agrees_with_the_count() {
        assert_eq!(
            plan_step_label(1, tact_catalog::CatalogLocale::En),
            "PLAN / 1 STEP"
        );
        assert_eq!(
            plan_step_label(4, tact_catalog::CatalogLocale::En),
            "PLAN / 4 STEPS"
        );
        assert_eq!(
            plan_step_label(1, tact_catalog::CatalogLocale::Zh),
            "计划 / 1 步"
        );
        assert_eq!(
            plan_status_label(true, tact_catalog::CatalogLocale::Zh),
            "执行中"
        );
        assert_eq!(
            plan_status_label(false, tact_catalog::CatalogLocale::En),
            "COMPLETE"
        );
    }

    #[test]
    fn new_activity_only_tracks_unseen_running_content() {
        assert!(!next_new_activity(false, true, true, false, true, false));
        assert!(next_new_activity(false, true, true, true, true, false));
        assert!(next_new_activity(false, false, false, true, true, false));
        assert!(!next_new_activity(false, false, true, true, true, false));
        assert!(!next_new_activity(true, true, true, true, false, false));
        assert!(!next_new_activity(true, true, true, true, true, true));
        assert!(next_new_activity(true, true, true, false, true, false));
    }

    #[test]
    fn gate_prompt_command_reads_the_quoted_command() {
        assert_eq!(
            gate_prompt_command("Allow Tact to run `cargo test --workspace`?").as_deref(),
            Some("cargo test --workspace")
        );
        assert_eq!(gate_prompt_command("Allow this action?"), None);
        assert_eq!(gate_prompt_command("Run `  `?"), None);
    }

    #[test]
    fn gate_help_matches_the_tool_kind() {
        use tact_desktop_core::CatalogLocale::{En, Zh};
        assert!(
            gate_description(En, Some("bash"))
                .unwrap()
                .contains("command")
        );
        assert!(
            gate_description(En, Some("worktree_run"))
                .unwrap()
                .contains("command")
        );
        assert!(
            gate_description(En, Some("background_run"))
                .unwrap()
                .contains("command")
        );
        assert!(
            gate_description(En, Some("ask_user"))
                .unwrap()
                .contains("question")
        );
        assert!(
            gate_description(En, Some("spawn_subagent"))
                .unwrap()
                .contains("subagent")
        );
        assert_eq!(gate_description(En, Some("read_file")), None);
        assert_eq!(gate_description(En, None), None);
        assert!(gate_description(Zh, Some("ask_user")).is_some());
    }

    #[test]
    fn tool_only_plans_yield_to_the_tool_rows() {
        let base = tact_desktop_core::fixtures::demo_transcript();
        let rows = base
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                if matches!(row.body, RowBody::Plan(_)) {
                    row.body = RowBody::Plan(tact_desktop_core::PlanRow {
                        title: Arc::from("Execution plan"),
                        steps: Arc::from([tact_desktop_core::PlanStep {
                            tool_id: Some(Arc::from("tool-4")),
                            label: Arc::from("cargo test -p tact store::session_store"),
                            status: RowStatus::Success,
                            detail: Some(Arc::from("shell")),
                        }]),
                    });
                }
                Arc::new(row)
            })
            .collect::<Vec<_>>();
        let transcript = TranscriptView::from_rows(rows);
        let plan_row = transcript
            .iter()
            .find(|row| matches!(row.body, RowBody::Plan(_)))
            .expect("plan row");
        let RowBody::Plan(plan) = &plan_row.body else {
            unreachable!()
        };
        assert!(
            plan_is_tool_execution(plan_row, plan, &transcript),
            "a one-step plan that is the tool call must not draw a second card"
        );

        let mut multi = (**plan_row).clone();
        multi.body = RowBody::Plan(tact_desktop_core::PlanRow {
            title: Arc::from("Execution plan"),
            steps: Arc::from([
                tact_desktop_core::PlanStep {
                    tool_id: Some(Arc::from("tool-4")),
                    label: Arc::from("Run the store tests"),
                    status: RowStatus::Success,
                    detail: Some(Arc::from("shell")),
                },
                tact_desktop_core::PlanStep {
                    tool_id: Some(Arc::from("tool-5")),
                    label: Arc::from("Edit the store"),
                    status: RowStatus::Running,
                    detail: Some(Arc::from("edit")),
                },
            ]),
        });
        let RowBody::Plan(multi_plan) = &multi.body else {
            unreachable!()
        };
        assert!(
            plan_is_tool_execution(&multi, multi_plan, &transcript),
            "a multi-step plan that only repeats tool rows must not draw a second card"
        );

        let mut conceptual = (**plan_row).clone();
        conceptual.body = RowBody::Plan(tact_desktop_core::PlanRow {
            title: Arc::from("Execution plan"),
            steps: Arc::from([
                tact_desktop_core::PlanStep {
                    tool_id: None,
                    label: Arc::from("Inspect the store"),
                    status: RowStatus::Success,
                    detail: None,
                },
                tact_desktop_core::PlanStep {
                    tool_id: Some(Arc::from("tool-4")),
                    label: Arc::from("Run the tests"),
                    status: RowStatus::Running,
                    detail: Some(Arc::from("shell")),
                },
            ]),
        });
        let RowBody::Plan(conceptual_plan) = &conceptual.body else {
            unreachable!()
        };
        assert!(
            !plan_is_tool_execution(&conceptual, conceptual_plan, &transcript),
            "a conceptual step keeps the plan card"
        );
    }

    #[test]
    fn avatars_use_two_uppercase_initials() {
        assert_eq!(initials_from_name("rg"), "RG");
        assert_eq!(initials_from_name(""), "Y");
        assert_eq!(initials_from_name("rg.agent"), "RG");
    }

    #[test]
    fn project_groups_share_the_repository_directory_identity() {
        let project = tact_desktop_core::ProjectView {
            id: Some(ProjectId("p".into())),
            root: Some("/tmp/gateway".into()),
            display_name: "gateway".into(),
            branch: None,
            repository: Some("acme/gateway".into()),
            available: true,
        };
        let label = project_identity_label(&project);
        assert_eq!(label, "acme/gateway");
        assert_eq!(project_initials(&label), "AG");

        let local = tact_desktop_core::ProjectView {
            repository: None,
            display_name: "dotfiles".into(),
            ..project
        };
        assert_eq!(project_identity_label(&local), "dotfiles");
        assert_eq!(project_initials("dotfiles"), "D");
    }

    #[test]
    fn run_titles_drop_a_leading_url() {
        assert_eq!(
            strip_leading_url("https://labcoder.github.io/gpui-ai 分析下这个库"),
            "分析下这个库"
        );
        assert_eq!(strip_leading_url("  plain title"), "  plain title");
    }

    #[test]
    fn process_summary_counts_commands_files_and_skills() {
        let group = ProcessGroup {
            first_index: 0,
            end_index: 4,
            first_row: RowId::new(0),
            first_ordinal: 0,
            commands: 25,
            files_read: 12,
            files_changed: 0,
            skills: 1,
            other_tools: 0,
            thoughts: 3,
            active: false,
        };
        assert_eq!(
            process_summary_text(&group, tact_catalog::CatalogLocale::En),
            "3 thoughts, ran 25 commands, read 12 files, ran 1 skill"
        );
        assert_eq!(
            process_summary_text(&group, tact_catalog::CatalogLocale::Zh),
            "3 段思考，运行了 25 条命令，读取了 12 个文件，运行了 1 个 skill"
        );
        let mut without_thoughts = group.clone();
        without_thoughts.thoughts = 0;
        assert_eq!(
            process_summary_text(&without_thoughts, tact_catalog::CatalogLocale::En),
            "Ran 25 commands, read 12 files, ran 1 skill"
        );
        assert_eq!(
            process_summary_text(&without_thoughts, tact_catalog::CatalogLocale::Zh),
            "运行了 25 条命令，读取了 12 个文件，运行了 1 个 skill"
        );
    }

    #[test]
    fn commentary_assistant_text_joins_the_process_group() {
        use tact_desktop_core::transcript::AssistantRow;
        let assistant = |phase| {
            RowBody::Assistant(AssistantRow {
                markdown: Arc::from("note"),
                status: RowStatus::Success,
                suggestions: Arc::from([]),
                phase,
            })
        };
        assert!(process_row_groupable(&assistant(Some(
            MessagePhase::Commentary
        ))));
        assert!(is_process_row(&assistant(Some(MessagePhase::Commentary))));
        assert!(!process_row_groupable(&assistant(Some(
            MessagePhase::FinalAnswer
        ))));
        assert!(!process_row_groupable(&assistant(None)));
    }

    #[test]
    fn answers_only_keeps_pending_gates_and_errors() {
        use tact_desktop_core::{ToolGroupItem, ToolGroupRow, transcript::ErrorRow};
        let gate = tact_desktop_core::fixtures::demo_gate();
        assert!(answers_only_visible(&RowBody::Gate(gate.clone())));
        let loop_id = tact_desktop_core::fixtures::demo_transcript()
            .iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => Some(group.loop_id),
                _ => None,
            })
            .expect("demo tool group");
        assert!(answers_only_visible(&RowBody::ToolGroup(ToolGroupRow {
            loop_id,
            items: Arc::from([ToolGroupItem::Gate(gate)]),
            summary: Arc::from("permission"),
            collapsed: false,
            collapse_override: None,
        })));
        assert!(answers_only_visible(&RowBody::Error(ErrorRow {
            message: Arc::from("boom"),
            retryable: false,
        })));
    }

    #[gpui::test]
    fn hidden_tool_plans_do_not_split_the_activity_group(cx: &mut TestAppContext) {
        use tact_desktop_core::{
            PlanRow, PlanStep, ToolBody, ToolGroupItem, ToolGroupRow, ToolMetaView, ToolOutputView,
            ToolRow,
            transcript::{AssistantRow, ThinkingRow},
        };

        fn row(
            id: u64,
            loop_id: Option<tact_desktop_core::LoopId>,
            body: RowBody,
        ) -> Arc<TranscriptRow> {
            Arc::new(TranscriptRow {
                id: RowId::new(id),
                ordinal: id,
                loop_id,
                time: None,
                body,
            })
        }

        fn thinking() -> RowBody {
            RowBody::Thinking(ThinkingRow {
                markdown: Arc::from("thinking"),
                status: RowStatus::Success,
                duration_ms: None,
            })
        }

        fn tool_plan(tool_id: &str) -> RowBody {
            RowBody::Plan(PlanRow {
                title: Arc::from("plan"),
                steps: Arc::from([PlanStep {
                    tool_id: Some(Arc::from(tool_id)),
                    label: Arc::from("run"),
                    status: RowStatus::Success,
                    detail: None,
                }]),
            })
        }

        fn tool_group(loop_id: tact_desktop_core::LoopId, id: u64, tool_id: &str) -> RowBody {
            RowBody::ToolGroup(ToolGroupRow {
                loop_id,
                items: Arc::from([ToolGroupItem::Tool(ToolRow {
                    id: RowId::new(id + 100),
                    tool_id: Arc::from(tool_id),
                    name: Arc::from("bash"),
                    arg_summary: Arc::from("echo hi"),
                    arg_full: None,
                    status: RowStatus::Success,
                    body: ToolBody::None,
                    output: ToolOutputView::default(),
                    meta: ToolMetaView::default(),
                    subagent: None,
                    expanded: false,
                    keep_live: false,
                    destructive: false,
                    duration_ms: None,
                })]),
                summary: Arc::from("bash"),
                collapsed: true,
                collapse_override: None,
            })
        }

        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1200.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let loop_id = demo
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => Some(group.loop_id),
                        _ => None,
                    })
                    .expect("demo tool group");
                let rows = vec![
                    row(0, Some(loop_id), thinking()),
                    row(1, Some(loop_id), tool_plan("tool-1")),
                    row(2, Some(loop_id), tool_group(loop_id, 2, "tool-1")),
                    row(3, Some(loop_id), thinking()),
                    row(4, Some(loop_id), tool_plan("tool-2")),
                    row(5, Some(loop_id), tool_group(loop_id, 5, "tool-2")),
                    row(
                        6,
                        Some(loop_id),
                        RowBody::Assistant(AssistantRow {
                            markdown: Arc::from("answer"),
                            status: RowStatus::Success,
                            suggestions: Arc::from(Vec::<Arc<str>>::new()),
                            phase: None,
                        }),
                    ),
                ];
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = TranscriptView::from_rows(rows);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        let groups = window.update(|_, cx| {
            root.read(cx)
                .transcript_scroller
                .read(cx)
                .process_groups
                .clone()
        });
        assert_eq!(
            groups.len(),
            1,
            "hidden tool plans must not split the activity group"
        );
        assert_eq!(groups[0].thoughts, 2);
        assert_eq!(groups[0].commands, 2);

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn commentary_text_joins_the_activity_group_but_the_answer_stays_out(cx: &mut TestAppContext) {
        use tact_desktop_core::{
            ToolBody, ToolGroupItem, ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow,
            transcript::{AssistantRow, ThinkingRow},
        };

        fn row(id: u64, body: RowBody) -> Arc<TranscriptRow> {
            Arc::new(TranscriptRow {
                id: RowId::new(id),
                ordinal: id,
                loop_id: None,
                time: None,
                body,
            })
        }

        fn assistant(text: &str, phase: MessagePhase) -> RowBody {
            RowBody::Assistant(AssistantRow {
                markdown: Arc::from(text),
                status: RowStatus::Success,
                suggestions: Arc::from([]),
                phase: Some(phase),
            })
        }

        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let loop_id = tact_desktop_core::fixtures::demo_transcript()
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => Some(group.loop_id),
                        _ => None,
                    })
                    .expect("demo tool group");
                let rows = vec![
                    row(
                        0,
                        RowBody::Thinking(ThinkingRow {
                            markdown: Arc::from("plan"),
                            status: RowStatus::Success,
                            duration_ms: None,
                        }),
                    ),
                    row(1, assistant("checking the store", MessagePhase::Commentary)),
                    row(
                        2,
                        RowBody::ToolGroup(ToolGroupRow {
                            loop_id,
                            items: Arc::from([ToolGroupItem::Tool(ToolRow {
                                id: RowId::new(12),
                                tool_id: Arc::from("tool-1"),
                                name: Arc::from("bash"),
                                arg_summary: Arc::from("echo hi"),
                                arg_full: None,
                                status: RowStatus::Success,
                                body: ToolBody::None,
                                output: ToolOutputView::default(),
                                meta: ToolMetaView::default(),
                                subagent: None,
                                expanded: false,
                                keep_live: false,
                                destructive: false,
                                duration_ms: None,
                            })]),
                            summary: Arc::from("bash"),
                            collapsed: true,
                            collapse_override: None,
                        }),
                    ),
                    row(3, assistant("done", MessagePhase::FinalAnswer)),
                ];
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = TranscriptView::from_rows(rows);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        let (groups, roles) = window.update(|_, cx| {
            let scroller = root.read(cx).transcript_scroller.read(cx);
            (
                scroller.process_groups.clone(),
                scroller.process_roles.clone(),
            )
        });
        assert_eq!(groups.len(), 1, "commentary must stay in one process group");
        assert_eq!(groups[0].thoughts, 1);
        assert_eq!(groups[0].commands, 1);
        assert_eq!(
            roles[1],
            ProcessRowRole::Hidden(0),
            "the narration row is part of the group"
        );
        assert_eq!(
            roles[3],
            ProcessRowRole::Normal,
            "the final answer stays outside the group"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn answers_only_hides_process_rows_and_keeps_the_answer(cx: &mut TestAppContext) {
        use tact_desktop_core::{
            ToolBody, ToolGroupItem, ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow,
            transcript::{AssistantRow, ThinkingRow, UserRow},
        };

        fn row(id: u64, body: RowBody) -> Arc<TranscriptRow> {
            Arc::new(TranscriptRow {
                id: RowId::new(id),
                ordinal: id,
                loop_id: None,
                time: None,
                body,
            })
        }

        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1400.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let loop_id = tact_desktop_core::fixtures::demo_transcript()
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => Some(group.loop_id),
                        _ => None,
                    })
                    .expect("demo tool group");
                let rows = vec![
                    row(
                        0,
                        RowBody::User(UserRow {
                            text: Arc::from("question"),
                            attachments: Arc::from([]),
                        }),
                    ),
                    row(
                        1,
                        RowBody::Thinking(ThinkingRow {
                            markdown: Arc::from("plan"),
                            status: RowStatus::Success,
                            duration_ms: None,
                        }),
                    ),
                    row(
                        2,
                        RowBody::ToolGroup(ToolGroupRow {
                            loop_id,
                            items: Arc::from([ToolGroupItem::Tool(ToolRow {
                                id: RowId::new(12),
                                tool_id: Arc::from("tool-1"),
                                name: Arc::from("bash"),
                                arg_summary: Arc::from("echo hi"),
                                arg_full: None,
                                status: RowStatus::Success,
                                body: ToolBody::None,
                                output: ToolOutputView::default(),
                                meta: ToolMetaView::default(),
                                subagent: None,
                                expanded: false,
                                keep_live: false,
                                destructive: false,
                                duration_ms: None,
                            })]),
                            summary: Arc::from("bash"),
                            collapsed: true,
                            collapse_override: None,
                        }),
                    ),
                    row(
                        3,
                        RowBody::Assistant(AssistantRow {
                            markdown: Arc::from("checking"),
                            status: RowStatus::Success,
                            suggestions: Arc::from([]),
                            phase: Some(MessagePhase::Commentary),
                        }),
                    ),
                    row(
                        4,
                        RowBody::Assistant(AssistantRow {
                            markdown: Arc::from("answer"),
                            status: RowStatus::Success,
                            suggestions: Arc::from([]),
                            phase: Some(MessagePhase::FinalAnswer),
                        }),
                    ),
                ];
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = TranscriptView::from_rows(rows);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        let toggle = window
            .debug_bounds("run-tab-answers-only")
            .expect("answers-only toggle");
        window.simulate_click(toggle.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            window.debug_bounds("transcript-row-user-0").is_some(),
            "the user's question stays"
        );
        assert!(
            window.debug_bounds("transcript-row-assistant-4").is_some(),
            "the final answer stays"
        );
        assert!(
            window.debug_bounds("transcript-row-thinking-1").is_none()
                && window.debug_bounds("transcript-row-tool-group-2").is_none()
                && window.debug_bounds("transcript-row-assistant-3").is_none(),
            "process rows are hidden"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn the_process_summary_label_switches_from_live_to_counts() {
        let group = ProcessGroup {
            first_index: 0,
            end_index: 2,
            first_row: RowId::new(0),
            first_ordinal: 0,
            commands: 4,
            files_read: 0,
            files_changed: 0,
            skills: 0,
            other_tools: 0,
            thoughts: 3,
            active: true,
        };
        assert_eq!(
            process_summary_label(
                &group,
                Some("Running · edit_file bridge.rs"),
                Some("2s"),
                tact_catalog::CatalogLocale::En,
            ),
            "2s · Running · edit_file bridge.rs"
        );
        let mut settled = group.clone();
        settled.active = false;
        assert_eq!(
            process_summary_label(&settled, None, Some("2s"), tact_catalog::CatalogLocale::En),
            "2s · 3 thoughts, ran 4 commands"
        );
    }

    fn summary_row(id: u64, body: RowBody) -> Arc<TranscriptRow> {
        Arc::new(TranscriptRow {
            id: RowId::new(id),
            ordinal: id,
            loop_id: None,
            time: None,
            body,
        })
    }

    fn summary_thinking(status: RowStatus) -> RowBody {
        RowBody::Thinking(tact_desktop_core::transcript::ThinkingRow {
            markdown: Arc::from("plan"),
            status,
            duration_ms: None,
        })
    }

    fn summary_tool_group(loop_id: tact_desktop_core::LoopId, status: RowStatus) -> RowBody {
        use tact_desktop_core::{
            ToolBody, ToolGroupItem, ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow,
        };
        RowBody::ToolGroup(ToolGroupRow {
            loop_id,
            items: Arc::from([ToolGroupItem::Tool(ToolRow {
                id: RowId::new(12),
                tool_id: Arc::from("tool-1"),
                name: Arc::from("bash"),
                arg_summary: Arc::from("echo hi"),
                arg_full: None,
                status,
                body: ToolBody::None,
                output: ToolOutputView::default(),
                meta: ToolMetaView::default(),
                subagent: None,
                expanded: false,
                keep_live: false,
                destructive: false,
                duration_ms: None,
            })]),
            summary: Arc::from("bash"),
            collapsed: true,
            collapse_override: None,
        })
    }

    #[gpui::test]
    fn the_process_summary_shows_the_live_status_and_keeps_its_clock(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1400.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let loop_id = tact_desktop_core::fixtures::demo_transcript()
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => Some(group.loop_id),
                        _ => None,
                    })
                    .expect("demo tool group");
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                snapshot.runtime.current_phase = tact_desktop_core::RunPhase::RunningTools;
                snapshot.runtime.latest_tool_detail = Some(LatestToolView {
                    tool_id: "call-live".into(),
                    group: None,
                    row: None,
                    label: "Editing root.rs".into(),
                    duration_ms: None,
                    status: RowStatus::Running,
                });
                snapshot.transcript = TranscriptView::from_rows(vec![
                    summary_row(0, summary_thinking(RowStatus::Running)),
                    summary_row(1, summary_tool_group(loop_id, RowStatus::Pending)),
                ]);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        assert!(
            window
                .debug_bounds("transcript-process-summary-0")
                .is_some(),
            "the running status lives on the process summary"
        );
        let started_at = window.update(|_, cx| {
            root.read(cx)
                .transcript_scroller
                .read(cx)
                .group_started
                .get(&RowId::new(0))
                .copied()
        });
        assert!(started_at.is_some(), "an active group gets a clock");
        assert!(
            window.debug_bounds("composer-live-activity").is_none(),
            "the standalone composer chip is gone"
        );

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let loop_id = tact_desktop_core::fixtures::demo_transcript()
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => Some(group.loop_id),
                        _ => None,
                    })
                    .expect("demo tool group");
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = TranscriptView::from_rows(vec![
                    summary_row(0, summary_thinking(RowStatus::Success)),
                    summary_row(1, summary_tool_group(loop_id, RowStatus::Running)),
                ]);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            window.update(|_, cx| {
                root.read(cx)
                    .transcript_scroller
                    .read(cx)
                    .group_started
                    .get(&RowId::new(0))
                    .copied()
            }),
            started_at,
            "the clock spans thinking into tools without restarting"
        );

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let loop_id = tact_desktop_core::fixtures::demo_transcript()
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::ToolGroup(group) => Some(group.loop_id),
                        _ => None,
                    })
                    .expect("demo tool group");
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Ready;
                snapshot.transcript = TranscriptView::from_rows(vec![
                    summary_row(0, summary_thinking(RowStatus::Success)),
                    summary_row(1, summary_tool_group(loop_id, RowStatus::Success)),
                ]);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });

        let (started, elapsed) = window.update(|_, cx| {
            let scroller = root.read(cx).transcript_scroller.read(cx);
            (
                scroller.group_started.contains_key(&RowId::new(0)),
                scroller.group_elapsed.get(&RowId::new(0)).cloned(),
            )
        });
        assert!(!started, "the clock stops when the group settles");
        assert!(
            elapsed.is_some_and(|elapsed| !elapsed.is_empty()),
            "the settled summary keeps the elapsed time"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_process_summary_clock_survives_a_tool_batch_boundary(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(1400.)));
        let loop_id = tact_desktop_core::fixtures::demo_transcript()
            .iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => Some(group.loop_id),
                _ => None,
            })
            .expect("demo tool group");

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                snapshot.runtime.current_phase = tact_desktop_core::RunPhase::RunningTools;
                snapshot.transcript = TranscriptView::from_rows(vec![
                    summary_row(0, summary_thinking(RowStatus::Running)),
                    summary_row(1, summary_tool_group(loop_id, RowStatus::Pending)),
                ]);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });
        let started_at = window
            .update(|_, cx| {
                root.read(cx)
                    .transcript_scroller
                    .read(cx)
                    .group_started
                    .get(&RowId::new(0))
                    .copied()
            })
            .expect("an active group gets a clock");

        // The model is between requests: no row runs until the next thinking
        // chunk arrives, so the group goes idle for a moment.
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                snapshot.transcript = TranscriptView::from_rows(vec![
                    summary_row(0, summary_thinking(RowStatus::Success)),
                    summary_row(1, summary_tool_group(loop_id, RowStatus::Success)),
                ]);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            window.update(|_, cx| {
                root.read(cx)
                    .transcript_scroller
                    .read(cx)
                    .group_started
                    .get(&RowId::new(0))
                    .copied()
            }),
            Some(started_at),
            "an idle gap must not restart the group's clock"
        );

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                snapshot.runtime.current_phase = tact_desktop_core::RunPhase::Thinking;
                snapshot.transcript = TranscriptView::from_rows(vec![
                    summary_row(0, summary_thinking(RowStatus::Success)),
                    summary_row(1, summary_tool_group(loop_id, RowStatus::Success)),
                    summary_row(2, summary_thinking(RowStatus::Running)),
                ]);
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            window.update(|_, cx| {
                root.read(cx)
                    .transcript_scroller
                    .read(cx)
                    .group_started
                    .get(&RowId::new(0))
                    .copied()
            }),
            Some(started_at),
            "the clock keeps its original start when the group wakes up again"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_run_header_clock_repaints_while_a_run_is_active(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Running;
                snapshot.runtime.turn_started_at =
                    Some(chrono::Utc::now() - chrono::Duration::seconds(5));
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.update(|_, cx| root.read(cx).run_clock_timer.is_some()),
            "a running turn owns a one-second header clock"
        );

        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.session.phase = SessionPhase::Ready;
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.update(|_, cx| root.read(cx).run_clock_timer.is_none()),
            "the settled run drops its timer"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn chrome_matches_prototype_metrics(cx: &mut TestAppContext) {
        let (_root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let titlebar = window.debug_bounds("desktop-title-bar").expect("title bar");
        // The prototype's 58px includes its 1px bottom border; debug bounds
        // report the content box.
        assert_eq!(titlebar.size.height + px(1.), px(58.), "titlebar height");

        let rail = window.debug_bounds("session-rail").expect("session rail");
        assert_eq!(rail.size.width, px(250.), "rail width");

        let area = window.debug_bounds("composer-area").expect("composer area");
        let composer = window
            .debug_bounds("composer-frame")
            .expect("composer frame");
        assert_eq!(composer.size.width, px(900.), "composer width");
        assert_eq!(
            composer.origin.x,
            area.origin.x + (area.size.width - px(900.)) / 2.,
            "composer x"
        );

        let sticky = window.debug_bounds("sticky-strip").expect("sticky strip");
        assert_eq!(sticky.size.width, px(900.), "sticky width");
        assert_eq!(
            sticky.origin.x,
            area.origin.x + (area.size.width - px(900.)) / 2.,
            "sticky x"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn run_header_sits_close_to_the_chat_tabs(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
            window.draw(cx).clear(cx);
        });
        let tabs = window.debug_bounds("run-tabs").expect("run tabs");
        let meta = window
            .debug_bounds("transcript-run-meta")
            .expect("run header meta");
        assert!(
            meta.origin.y - tabs.bottom() <= px(28.),
            "the run header must not leave a large gap under the tabs: tabs={tabs:?}, meta={meta:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn panel_dividers_resize_the_rail_and_the_inspector(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1440.), px(900.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.left_visible = true;
                this.right_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });

        let rail = window.debug_bounds("session-rail").expect("session rail");
        let rail_before = rail.size.width;
        let boundary = window
            .debug_bounds("rail-divider")
            .expect("rail divider")
            .center();
        window.simulate_mouse_down(
            boundary,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        window.simulate_mouse_move(
            boundary + point(px(60.), px(0.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        window.simulate_mouse_up(
            boundary + point(px(60.), px(0.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(window);
        let rail_after = window
            .debug_bounds("session-rail")
            .expect("session rail after drag")
            .size
            .width;
        assert!(
            rail_after > rail_before + px(40.),
            "dragging the left divider must widen the rail: {rail_before:?} -> {rail_after:?}"
        );

        let inspector = window.debug_bounds("run-inspector").expect("run inspector");
        let inspector_before = inspector.size.width;
        let boundary = window
            .debug_bounds("inspector-divider")
            .expect("inspector divider")
            .center();
        window.simulate_mouse_down(
            boundary,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        window.simulate_mouse_move(
            boundary - point(px(60.), px(0.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        window.simulate_mouse_up(
            boundary - point(px(60.), px(0.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(window);
        let inspector_after = window
            .debug_bounds("run-inspector")
            .expect("run inspector after drag")
            .size
            .width;
        assert!(
            inspector_after > inspector_before + px(40.),
            "dragging the right divider must widen the inspector: \
             {inspector_before:?} -> {inspector_after:?}"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn composer_controls_live_inside_the_prompt_frame(cx: &mut TestAppContext) {
        let (_root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let editor = window
            .debug_bounds("prompt-bar-editor")
            .expect("prompt editor");
        let permission = window
            .debug_bounds("permission-picker")
            .expect("permission picker");
        let composer = window.debug_bounds("composer-area").expect("composer area");
        assert!(
            permission.origin.y >= editor.origin.y + editor.size.height,
            "permission chip sits above the prompt editor: editor={editor:?}, permission={permission:?}"
        );
        assert!(
            permission.origin.y + permission.size.height
                <= composer.origin.y + composer.size.height,
            "permission chip escapes the composer area: permission={permission:?}, composer={composer:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_session_list_takes_the_space_the_durable_block_used_to(cx: &mut TestAppContext) {
        let (_root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        // The durable block (heading + three rows) was removed because it
        // duplicated the inspector's Goals/Subagents/Background tabs. That
        // space belongs to the session list, so the list must be taller than
        // the old cap rather than merely no longer overlapping.
        let list = window
            .debug_bounds("session-list")
            .expect("session list bounds");
        // The list now takes its design share of the window (~32%) instead of
        // the 20% cap the durable block forced. Tie the assertion to that
        // share so it fails if the old cap comes back.
        let viewport = window.update(|window, _| window.viewport_size());
        assert!(
            list.size.height >= viewport.height * 0.30,
            "the session list should take about a third of the window, got {list:?} of {viewport:?}"
        );

        // The extensions block no longer competes with the session list; the
        // rail keeps only the Settings entry at its bottom.
        let view_all = window
            .debug_bounds("view-all-sessions")
            .expect("view all sessions bounds");
        assert!(
            window.debug_bounds("rail-extensions").is_none(),
            "extension entries belong to Settings, not the rail"
        );
        assert!(
            view_all.origin.y
                < window
                    .debug_bounds("open-settings")
                    .expect("rail settings row")
                    .origin
                    .y,
            "Settings stays below the session list"
        );

        // The duplicate rows are gone for good: a second copy of the
        // Goals/Subagents/Background links must not come back.
        for selector in [
            "rail-durable",
            "rail-goals",
            "rail-subagents",
            "rail-background",
        ] {
            assert!(
                window.debug_bounds(selector).is_none(),
                "{selector} duplicates an inspector tab"
            );
        }

        let settings = window
            .debug_bounds("open-settings")
            .expect("rail settings row");
        assert!(
            settings.origin.y + settings.size.height <= viewport.height,
            "removing the durable block must not push Settings off the rail"
        );

        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn session_rail_groups_sessions_by_project(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(900.)));
        window.update(|window, cx| {
            root.update(cx, install_two_project_sessions);
            window.draw(cx).clear(cx);
        });
        let first_header = window
            .debug_bounds("sidebar-nav-section-sessions-project-project-a")
            .expect("project A heading");
        let first_row = window
            .debug_bounds("sidebar-nav-item-session-a")
            .expect("project A session");
        let second_header = window
            .debug_bounds("sidebar-nav-section-sessions-project-project-b")
            .expect("project B heading");
        let second_row = window
            .debug_bounds("sidebar-nav-item-session-b")
            .expect("project B session");
        assert!(
            window
                .debug_bounds("sidebar-nav-section-leading-sessions-project-project-a")
                .is_some(),
            "project A's heading must carry the shared project avatar"
        );
        assert!(
            window
                .debug_bounds("sidebar-nav-section-count-sessions-project-project-a")
                .is_some(),
            "project A's heading must carry its session count"
        );
        assert!(
            first_header.origin.y < first_row.origin.y
                && first_row.origin.y < second_header.origin.y
                && second_header.origin.y < second_row.origin.y,
            "sessions must sit under their project headings in recency order: {first_header:?}, {first_row:?}, {second_header:?}, {second_row:?}"
        );
        let children = window
            .debug_bounds("sidebar-nav-section-children-sessions-project-project-a")
            .expect("project A child tree");
        assert!(
            children.origin.x > first_header.origin.x && first_row.origin.x > first_header.origin.x,
            "session rows must indent under their project node: header={first_header:?}, children={children:?}, row={first_row:?}"
        );
        window.simulate_click(first_header.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("sidebar-nav-item-session-a").is_none(),
            "collapsing a project must hide its session children"
        );
        window.update(|_, cx| {
            assert!(
                root.read(cx)
                    .collapsed_session_projects
                    .contains(&tact_desktop_core::ProjectId("project-a".into())),
                "the rail must report the collapsed project"
            );
        });
        window.simulate_click(first_header.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("sidebar-nav-item-session-a").is_some(),
            "expanding a project must restore its session children"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn project_remove_entry_opens_a_confirmation(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(900.)));
        window.update(|window, cx| {
            root.update(cx, install_two_project_sessions);
            window.draw(cx).clear(cx);
        });
        let remove = window
            .debug_bounds("sidebar-nav-section-action-sessions-project-project-a")
            .expect("project remove action");
        window.simulate_click(remove.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("command-form-overlay").is_some(),
            "the project remove action must open a confirmation surface"
        );
        assert!(
            window.debug_bounds("form-choice-remove").is_some()
                && window.debug_bounds("form-choice-cancel").is_some(),
            "the confirmation must offer remove and cancel"
        );
        let remove_choice = window
            .debug_bounds("form-choice-remove")
            .expect("remove choice");
        window.simulate_click(remove_choice.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("command-form-overlay").is_none(),
            "confirming removal must dismiss the confirmation surface"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn session_manager_groups_sessions_by_project(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(900.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                install_two_project_sessions(this, cx);
                this.session_project_filter = None;
                this.open_overlay(Overlay::SessionManager);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let first_header = window
            .debug_bounds("manager-project-group-project-a")
            .expect("manager project A heading");
        let first_row = window
            .debug_bounds("manager-session-session-a")
            .expect("manager project A session");
        let second_header = window
            .debug_bounds("manager-project-group-project-b")
            .expect("manager project B heading");
        let second_row = window
            .debug_bounds("manager-session-session-b")
            .expect("manager project B session");
        assert!(
            window
                .debug_bounds("manager-project-group-leading-project-a")
                .is_some(),
            "manager project A heading must carry the shared project avatar"
        );
        assert!(
            window
                .debug_bounds("manager-project-group-count-project-a")
                .is_some(),
            "manager project A heading must carry its session count"
        );
        assert!(
            window
                .debug_bounds("manager-remove-project-project-a")
                .is_some(),
            "the manager project directory must expose a remove action"
        );
        assert!(
            window.debug_bounds("archive-session-session-a").is_some(),
            "the session manager must expose archive for each session"
        );
        assert!(
            first_header.origin.y < first_row.origin.y
                && first_row.origin.y < second_header.origin.y
                && second_header.origin.y < second_row.origin.y,
            "manager rows must sit under their project headings: {first_header:?}, {first_row:?}, {second_header:?}, {second_row:?}"
        );
        assert!(
            first_row.origin.x > first_header.origin.x,
            "manager session rows must indent under their project node"
        );
        window.simulate_click(first_header.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("manager-session-session-a").is_none(),
            "collapsing a manager project must hide its session children"
        );
        assert!(
            window.debug_bounds("manager-session-session-b").is_some(),
            "collapsing one project must leave other project nodes visible"
        );
        window.simulate_click(first_header.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("manager-session-session-a").is_some(),
            "expanding a manager project must restore its session children"
        );
        let project_b = window
            .debug_bounds("manager-project-project-b")
            .expect("project B filter");
        window.simulate_click(project_b.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("manager-session-session-a").is_none(),
            "the project filter must hide sessions from other projects"
        );
        assert!(
            window.debug_bounds("manager-session-session-b").is_some(),
            "the selected project's sessions must remain visible"
        );
        assert!(
            window.debug_bounds("session-manager-overlay").is_some(),
            "filtering by project must not close the manager"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn composer_has_no_project_directory_or_new_run_entry(cx: &mut TestAppContext) {
        let (_root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(900.)));
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("composer-frame").is_some(),
            "the composer itself must remain"
        );
        assert!(
            window.debug_bounds("composer-project").is_none(),
            "the composer must not repeat the project directory"
        );
        assert!(
            window.debug_bounds("open-project").is_none(),
            "the old rail project switcher must be gone"
        );
        assert!(
            window.debug_bounds("new-run").is_none(),
            "New Run must not be a standalone rail entry"
        );
        assert!(
            window.debug_bounds("composer-new-run").is_none(),
            "New Run must not be attached to the removed project bar"
        );
        assert!(
            window.debug_bounds("workspace-switcher").is_some(),
            "project switching remains available from the title bar"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn short_window_keeps_rail_settings_visible_at_150_percent(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(900.), px(700.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.text_scale = 1.5;
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let settings = window
            .debug_bounds("open-settings")
            .expect("rail settings row");
        let viewport = window.update(|window, _| window.viewport_size());
        assert!(
            settings.origin.y + settings.size.height <= viewport.height,
            "rail settings must stay visible: settings={settings:?}, viewport={viewport:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn sticky_strip_renders_a_task_card(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let card = window
            .debug_bounds("sticky-task-card")
            .expect("sticky task card");
        assert!(card.size.height > px(40.), "card={card:?}");
        assert!(
            window.debug_bounds("toggle-sticky-tasks").is_some(),
            "the card lost its collapse control"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn an_all_completed_quiet_board_hides_the_task_card(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector.tasks = std::sync::Arc::from([tact_desktop_core::TaskView {
                    id: 1,
                    subject: "done deal".into(),
                    status: tact_desktop_core::TaskStatusSnapshot::Completed,
                    ..tact_desktop_core::TaskView::default()
                }]);
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        // The TUI sticky hides once nothing is open; "TASKS 1/1" on its own is
        // not a reason to keep the card up.
        assert!(
            window.debug_bounds("sticky-task-card").is_none(),
            "a fully completed task list must not keep the card up"
        );

        // But any other domain the TUI strip would keep does: a running
        // background task summons the card back for its chip.
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector.background =
                    std::sync::Arc::from([tact_desktop_core::BackgroundTaskView {
                        status: tact_desktop_core::BackgroundTaskStatusSnapshot::Running,
                        ..tact_desktop_core::BackgroundTaskView::default()
                    }]);
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("sticky-task-card").is_some(),
            "a running background task keeps the card up for its chip"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn multistep_forms_restore_the_original_trigger_focus(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.prompt
                    .update(cx, |prompt, cx| prompt.focus(window, cx));
                this.remember_focus(window, cx);
                this.set_form(
                    FormState {
                        title: "Add MCP server".into(),
                        action: FormAction::McpAddTransport("probe".into()),
                        field: Some("URL".into()),
                        required: true,
                        choice: None,
                    },
                    window,
                    cx,
                );
                this.form_input.update(cx, |input, cx| {
                    input.set_value("https://example.com", window, cx)
                });
                this.submit_form(window, cx);
                this.choose_form_option("url".into(), window, cx);
                this.close_overlay();
                this.restore_focus(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        let (actual, trigger) = window.update(|window, cx| {
            (
                window.focused(cx),
                root.read(cx).prompt.read(cx).focus_handle(cx),
            )
        });
        assert_eq!(actual.as_ref(), Some(&trigger));
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn localized_component_labels_are_not_english(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        type CapturedLabels = Arc<std::sync::Mutex<(Option<String>, Option<String>)>>;
        struct LabelProbe {
            captured: CapturedLabels,
        }
        impl Render for LabelProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let captured = self.captured.clone();
                gpui::canvas(
                    move |_, window, cx| {
                        let strip = gpui::RenderOnce::render(
                            AttachmentStrip::new("localized-attachment-label")
                                .label(i18n::text(
                                    tact_catalog::CatalogLocale::Zh,
                                    TextKey::Attachment,
                                ))
                                .items([AttachmentItem::new("notes.md", "notes.md")]),
                            window,
                            cx,
                        )
                        .into_element();
                        let usage = ContextUsage::new(100, 1_000);
                        let meter = gpui::RenderOnce::render(
                            ContextMeter::new("localized-context-label", &usage).label(i18n::text(
                                tact_catalog::CatalogLocale::Zh,
                                TextKey::Context,
                            )),
                            window,
                            cx,
                        )
                        .into_element();
                        let mut strip_node = gpui::accesskit::Node::new(Role::Unknown);
                        let mut meter_node = gpui::accesskit::Node::new(Role::Unknown);
                        strip.write_a11y_info(&mut strip_node);
                        meter.write_a11y_info(&mut meter_node);
                        *captured.lock().unwrap() = (
                            strip_node.label().map(ToOwned::to_owned),
                            meter_node.label().map(ToOwned::to_owned),
                        );
                    },
                    |_, _, _, _| {},
                )
            }
        }
        let captured = CapturedLabels::default();
        let probe_capture = captured.clone();
        let (_, window) = cx.add_window_view(move |_, _| LabelProbe {
            captured: probe_capture,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let captured = captured.lock().unwrap().clone();
        assert_eq!(
            (captured.0.as_deref(), captured.1.as_deref()),
            (Some("附件"), Some("上下文"))
        );
    }

    #[gpui::test]
    fn localized_answer_and_suggestion_labels_are_not_english(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        type CapturedLabels = Arc<std::sync::Mutex<(Option<String>, Option<String>)>>;
        struct LabelProbe {
            captured: CapturedLabels,
        }
        impl Render for LabelProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let captured = self.captured.clone();
                gpui::canvas(
                    move |_, window, cx| {
                        let markdown: Arc<str> = "已完成的回复".into();
                        let content = streamed_text(
                            &markdown,
                            RowStatus::Success,
                            tact_catalog::CatalogLocale::Zh,
                        );
                        let answer = gpui::RenderOnce::render(
                            StreamingText::new("localized-answer", &content).label(i18n::text(
                                tact_catalog::CatalogLocale::Zh,
                                TextKey::AssistantResponse,
                            )),
                            window,
                            cx,
                        )
                        .into_element();
                        let suggestions = gpui::RenderOnce::render(
                            Suggestions::new("localized-suggestions")
                                .label(i18n::text(
                                    tact_catalog::CatalogLocale::Zh,
                                    TextKey::Suggestions,
                                ))
                                .items([Suggestion::new("follow-up", "继续")]),
                            window,
                            cx,
                        )
                        .into_element();
                        let mut answer_node = gpui::accesskit::Node::new(Role::Unknown);
                        let mut suggestions_node = gpui::accesskit::Node::new(Role::Unknown);
                        answer.write_a11y_info(&mut answer_node);
                        suggestions.write_a11y_info(&mut suggestions_node);
                        *captured.lock().unwrap() = (
                            answer_node.label().map(ToOwned::to_owned),
                            suggestions_node.label().map(ToOwned::to_owned),
                        );
                    },
                    |_, _, _, _| {},
                )
            }
        }
        let captured = CapturedLabels::default();
        let probe_capture = captured.clone();
        let (_, window) = cx.add_window_view(move |_, _| LabelProbe {
            captured: probe_capture,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let captured = captured.lock().unwrap().clone();
        assert_eq!(
            (captured.0.as_deref(), captured.1.as_deref()),
            (Some("助手回复"), Some("建议"))
        );
    }

    #[gpui::test]
    fn command_center_fits_a_short_window(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(600.), px(300.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| this.open_command_center(window, cx));
            window.draw(cx).clear(cx);
        });
        let search = window
            .debug_bounds("command-search-tact-command-center")
            .expect("command center search");
        assert!(
            search.origin.y >= px(0.) && search.origin.y + search.size.height <= px(300.),
            "command center search escapes a 600x300 window: {search:?}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    /// A long plan stays inside the strip: the window paints at most
    /// `sticky::MAX_VISIBLE_ROWS` rows and reports the rest in one marker row,
    /// the way the TUI sticky host does.
    #[gpui::test]
    fn a_long_task_list_scrolls_inside_the_strip(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                let mut inspector = tact_desktop_core::fixtures::demo_inspector();
                inspector.tasks = std::sync::Arc::from(
                    (1..=14u64)
                        .map(|id| tact_desktop_core::TaskView {
                            id,
                            subject: format!("step {id}").into(),
                            status: tact_desktop_core::TaskStatusSnapshot::Pending,
                            created_at: Some(id as i64),
                            ..tact_desktop_core::TaskView::default()
                        })
                        .collect::<Vec<_>>(),
                );
                snapshot.inspector = inspector;
                this.snapshot = std::sync::Arc::new(snapshot);
                this.sticky_tasks_collapsed = false;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });

        // Pending sorts newest first, so the window opens on 14 and stops at 5.
        assert!(
            window.debug_bounds("sticky-task-row-14").is_some(),
            "the newest task is in the window"
        );
        assert!(
            window.debug_bounds("sticky-task-row-1").is_none(),
            "the oldest task is past the window, not painted over the transcript"
        );
        assert!(
            window.debug_bounds("sticky-tasks-more").is_some(),
            "the marker row reports the remainder"
        );

        // Scrolling swaps which rows are painted and flips the marker, rather
        // than growing the strip.
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.sticky_tasks_scroll = 4;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("sticky-task-row-1").is_some(),
            "the oldest task is reachable"
        );
        assert!(
            window.debug_bounds("sticky-task-row-14").is_none(),
            "the newest scrolled out of the window"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn sticky_tasks_expand_grouped_with_open_work_first(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("sticky-task-row-2").is_none(),
            "the collapsed strip shows the current task in its header only"
        );
        let toggle = window
            .debug_bounds("toggle-sticky-tasks")
            .expect("sticky task toggle");
        window.simulate_click(toggle.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("sticky-task-row-2").is_some(),
            "expanding the strip renders the task rows"
        );

        // The demo inspector is one task per status: 1 done, 2 in progress,
        // 3 pending. In progress paints above pending, and the completed task
        // is hidden the same way the TUI sticky hides it.
        // `debug_bounds` takes a `&'static str`, so the probes are spelled out
        // rather than built from a loop.
        let doing = window
            .debug_bounds("sticky-task-row-2")
            .expect("in progress");
        let todo = window.debug_bounds("sticky-task-row-3").expect("pending");
        assert!(
            doing.origin.y < todo.origin.y,
            "in progress paints above pending: {doing:?} {todo:?}"
        );
        assert!(
            window.debug_bounds("sticky-task-row-1").is_none(),
            "completed work stays out of the strip; the n/m counter reports it"
        );
        // Both painted groups are labelled, so the split is readable rather
        // than two unannounced blocks.
        for selector in ["sticky-task-group-InProgress", "sticky-task-group-Pending"] {
            assert!(
                window.debug_bounds(selector).is_some(),
                "{selector} header is painted"
            );
        }
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn sticky_background_opens_a_task_popover(cx: &mut TestAppContext) {
        struct StickyProbe {
            root: Entity<DesktopRoot>,
        }
        impl Render for StickyProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.root.update(cx, |_, cx| {
                    v_flex()
                        .id("sticky-host")
                        .debug_selector(|| "sticky-host".into())
                        .size(px(240.))
                        .items_center()
                        .justify_center()
                        .child(sticky_shortcut("Background", InspectorTab::Background, cx))
                })
            }
        }
        let data_dir = probe_dir("sticky-background");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let (probe, window) = cx.add_window_view(move |window, cx| {
            gpui_component::init(cx);
            crate::theme::apply(DesktopTheme::Tact, Appearance::Light, Some(window), cx);
            let root = cx.new(|cx| DesktopRoot::new_for_test(controller, window, cx));
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.inspector.background =
                    Arc::from([tact_desktop_core::BackgroundTaskView {
                        id: Arc::from("task-1"),
                        command: Arc::from("sleep 10"),
                        status: tact_desktop_core::BackgroundTaskStatusSnapshot::Running,
                        ..tact_desktop_core::BackgroundTaskView::default()
                    }]);
                this.snapshot = Arc::new(snapshot);
                cx.notify();
            });
            StickyProbe { root }
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = window.debug_bounds("sticky-host").expect("sticky probe");
        window.simulate_click(bounds.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        // The strip's Background shortcut opens the Background tab rather
        // than a dialog: one surface per data set, and it lives beside the
        // other inspector facets.
        assert!(window.update(|_, cx| {
            let root = probe.read(cx).root.read(cx);
            root.overlay.is_none() && root.snapshot.inspector.tab == InspectorTab::Background
        }));
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }
    #[gpui::test]
    fn every_transcript_row_kind_renders(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(2600.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
                snapshot.session.phase = SessionPhase::Running;
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| state.reset(count, cx));
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.update(|_, cx| root.read(cx).transcript_rows.len()) >= 9,
            "the virtualized transcript must build the visible row kinds"
        );
        for selector in [
            "transcript-row-user-0",
            "transcript-row-thinking-2",
            "transcript-row-plan-13",
            "transcript-row-tool-group-3",
            "transcript-row-assistant-7",
            "transcript-row-diff-8",
        ] {
            assert!(
                window.debug_bounds(selector).is_some(),
                "row {selector} did not render"
            );
        }
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(12, cx));
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        for selector in [
            "transcript-row-notice-9",
            "transcript-row-hook-status-10",
            "transcript-row-error-11",
            "transcript-row-gate-12",
        ] {
            assert!(
                window.debug_bounds(selector).is_some(),
                "row {selector} did not render after scrolling to the tail"
            );
        }
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn tool_only_plan_rows_do_not_render_above_their_tool_group(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(2600.)));
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let demo = tact_desktop_core::fixtures::demo_transcript();
                let rows = demo
                    .iter()
                    .map(|row| {
                        let mut next = (**row).clone();
                        if let RowBody::Plan(plan) = &row.body {
                            next.body = RowBody::Plan(tact_desktop_core::PlanRow {
                                title: plan.title.clone(),
                                steps: Arc::from([
                                    tact_desktop_core::PlanStep {
                                        tool_id: Some(Arc::from("tool-4")),
                                        label: Arc::from("Run the store tests"),
                                        status: RowStatus::Success,
                                        detail: Some(Arc::from("shell")),
                                    },
                                    tact_desktop_core::PlanStep {
                                        tool_id: Some(Arc::from("tool-5")),
                                        label: Arc::from("Edit the store"),
                                        status: RowStatus::Running,
                                        detail: Some(Arc::from("edit")),
                                    },
                                ]),
                            });
                        }
                        Arc::new(next)
                    })
                    .collect::<Vec<_>>();
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = TranscriptView::from_rows(rows);
                snapshot.session.phase = SessionPhase::Running;
                this.snapshot = Arc::new(snapshot);
                this.pending_row_updates
                    .extend(this.snapshot.transcript.iter().map(|row| row.id));
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| state.reset(count, cx));
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("transcript-row-plan-13").is_none(),
            "a tool-only plan must not render above the same tool rows"
        );
        assert!(
            window.debug_bounds("transcript-row-tool-group-3").is_some(),
            "the execution group remains the canonical tool surface"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn the_model_picker_keeps_its_slot_while_the_composer_initialises(cx: &mut TestAppContext) {
        fn trigger_x(
            window: &mut VisualTestContext,
            root: &Entity<DesktopRoot>,
            models: &[&str],
            model: &str,
            phase: SessionPhase,
        ) -> f32 {
            window.update(|_, cx| {
                root.update(cx, |this, cx| {
                    let mut snapshot = (*this.snapshot).clone();
                    snapshot.composer.models = models
                        .iter()
                        .map(|model| Arc::from(*model))
                        .collect::<Vec<_>>()
                        .into();
                    snapshot.composer.model = Arc::from(model);
                    snapshot.session.phase = phase;
                    this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                });
            });
            draw(window);
            window
                .debug_bounds("prompt-bar-model-trigger")
                .expect("model trigger renders")
                .origin
                .x
                .as_f32()
        }

        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        draw(window);
        let initial = window
            .debug_bounds("prompt-bar-model-trigger")
            .expect("model trigger renders")
            .origin
            .x
            .as_f32();
        let model = "opencode-go/deepseek-v4.1-flash";
        let loaded = trigger_x(window, &root, &[model], model, SessionPhase::Ready);
        let starting = trigger_x(window, &root, &[model], model, SessionPhase::Starting);
        assert_eq!(
            initial, loaded,
            "the model trigger must not move when the model list loads: \
             initial={initial}, loaded={loaded}"
        );
        assert_eq!(
            loaded, starting,
            "the model trigger must not move while the session starts: \
             loaded={loaded}, starting={starting}"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn model_menu_scrolls_past_its_first_screenful(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|_, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                snapshot.composer.models = (0..28)
                    .map(|index| Arc::from(format!("model-{index:02}")))
                    .collect::<Vec<_>>()
                    .into();
                snapshot.composer.model = Arc::from("model-00");
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_item(2, cx));
            });
        });
        draw(window);
        let trigger = window
            .debug_bounds("prompt-bar-model-trigger")
            .expect("model trigger renders");
        window.simulate_click(trigger.center(), gpui::Modifiers::default());
        draw(window);
        let menu = window
            .debug_bounds("prompt-bar-model-picker")
            .expect("model menu opens");
        assert!(
            menu.size.height <= px(270.) && menu.bottom() <= trigger.top(),
            "the menu stays capped and flips above the composer: menu={menu:?}, trigger={trigger:?}"
        );
        let transcript_before = window
            .debug_bounds("transcript-row-thinking-2")
            .expect("transcript row behind the menu");
        let model_list = window
            .debug_bounds("prompt-bar-model-list")
            .expect("model rows render");
        let wheel_at = point(model_list.origin.x + px(20.), model_list.origin.y + px(20.));
        let transcript_view = window
            .debug_bounds("desktop-transcript")
            .expect("transcript viewport renders");
        assert!(
            transcript_view.contains(&wheel_at),
            "the wheel must land over the transcript for the isolation check to mean \
             anything: viewport={transcript_view:?}, wheel={wheel_at:?}"
        );
        assert!(
            window.debug_bounds("prompt-bar-model-search").is_some(),
            "a model list longer than eight rows needs its own search"
        );
        window.simulate_event(gpui::ScrollWheelEvent {
            position: wheel_at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-2000.))),
            ..Default::default()
        });
        draw(window);
        assert!(
            window
                .debug_bounds("prompt-bar-model-option-model-00")
                .is_none(),
            "row scrolling must window out rows above the viewport"
        );
        assert!(
            window
                .debug_bounds("prompt-bar-model-option-model-27")
                .is_some(),
            "the last model remains visible after row scrolling"
        );
        let search = window
            .debug_bounds("prompt-bar-model-search")
            .expect("model search field");
        window.simulate_click(search.center(), gpui::Modifiers::default());
        window.simulate_input("27");
        draw(window);
        assert!(
            window
                .debug_bounds("prompt-bar-model-option-model-27")
                .is_some()
                && window
                    .debug_bounds("prompt-bar-model-option-model-00")
                    .is_none(),
            "typing in the model search must filter the row window"
        );
        let last_after = window
            .debug_bounds("prompt-bar-model-option-model-27")
            .expect("the filtered model row");
        let transcript_after = window
            .debug_bounds("transcript-row-thinking-2")
            .expect("transcript row after scrolling");
        assert_eq!(
            transcript_after.origin.y, transcript_before.origin.y,
            "the wheel over the menu must not scroll the transcript behind it"
        );
        window.simulate_click(last_after.center(), gpui::Modifiers::default());
        draw(window);
        assert!(
            window.debug_bounds("prompt-bar-model-picker").is_none(),
            "an option reached by scrolling must still be clickable"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn composer_queue_scrolls_past_its_cap(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|_, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.composer.queue = (0..30)
                    .map(|index| tact_desktop_core::QueuedPromptView {
                        id: index,
                        text: Arc::from(format!("queued prompt {index}")),
                        attachments: Arc::from([]),
                    })
                    .collect::<Vec<_>>()
                    .into();
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                this.queue_expanded = true;
                cx.notify();
            });
        });
        draw(window);
        let at = window
            .debug_bounds("queue-item-composer-queue-0")
            .expect("first queued row renders")
            .center();
        assert_scrolls_past_cap(
            window,
            at,
            "queue-item-composer-queue-0",
            "queue-item-composer-queue-29",
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn hooks_list_scrolls_past_its_cap(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.simulate_resize(size(px(1280.), px(840.)));
        window.update(|_, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.surface = Some(SurfaceSnapshot::Hooks(tact_desktop_core::HookSnapshot {
                    trusted: (0..30)
                        .map(|index| tact_desktop_core::HookItemSnapshot {
                            source: format!("source-{index:02}"),
                            event: "post_tool".into(),
                            matcher: None,
                            command: format!("hook {index}"),
                        })
                        .collect(),
                    pending: Vec::new(),
                }));
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                this.open_overlay(Overlay::Hooks);
                cx.notify();
            });
        });
        draw(window);
        let at = window
            .debug_bounds("hook-source-source-00")
            .expect("first hook row renders")
            .center();
        assert_scrolls_past_cap(window, at, "hook-source-source-00", "hook-source-source-29");
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn mention_suggestions_scroll_past_their_cap(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        let project = probe_dir("mentions");
        for index in 0..30 {
            std::fs::write(project.join(format!("mention-{index:02}.txt")), "probe").unwrap();
        }
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.project.root = Some(project.clone());
                this.set_snapshot_for_capture(Arc::new(snapshot), cx);
                this.prompt
                    .update(cx, |prompt, cx| prompt.focus(window, cx));
            });
        });
        window.simulate_input("@mention");
        draw(window);
        let at = window
            .debug_bounds("prompt-bar-suggestion-0")
            .expect("first mention row renders")
            .center();
        window.simulate_event(gpui::ScrollWheelEvent {
            position: at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-2000.))),
            ..Default::default()
        });
        draw(window);
        assert!(
            window.debug_bounds("prompt-bar-suggestion-0").is_none(),
            "row scrolling must window out suggestions above the viewport"
        );
        assert!(
            window.debug_bounds("prompt-bar-suggestion-29").is_some(),
            "the last mention must remain reachable after row scrolling"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
        let _ = std::fs::remove_dir_all(project);
    }

    #[gpui::test]
    fn informational_toasts_hide_after_three_seconds(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                this.show_toast(
                    Toast {
                        title: "Settings".into(),
                        body: "Updated".into(),
                    },
                    true,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            window.debug_bounds("desktop-toast").is_some(),
            "the notification starts visible"
        );

        window
            .cx
            .background_executor
            .advance_clock(std::time::Duration::from_secs(4));
        window.run_until_parked();
        draw(window);

        assert!(
            window.debug_bounds("desktop-toast").is_none(),
            "informational notifications dismiss themselves after three seconds"
        );
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
