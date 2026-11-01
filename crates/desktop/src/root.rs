use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AppContext as _, ClickEvent, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder, Render, Role,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Task, WeakEntity, Window,
    canvas, div, point, prelude::FluentBuilder as _, px,
};
use gpui_ai::prelude::*;
use gpui_base::FocusTrapElement as _;
use gpui_component::message_scroller::{MessageScroller, MessageScrollerState};
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
    DesktopEvent, DesktopIntent, DesktopSnapshot, ExtensionsTab, GateCard, GateKind, GateResponse,
    GateSelection, GateState, InspectorTab, PermissionModeView, ProjectId, ReaderRef, RowBody,
    RowId, RowStatus, SectionMask, SessionId, SessionPhase, SettingValue, Submission,
    SurfaceSnapshot, ToolGroupItem, ToolRow, TranscriptRow, TranscriptView, is_enabled,
};

use crate::i18n::{self, TextKey};
use crate::ui::{ansi, diff_review, mermaid, web_search};
use crate::{keymap, theme};

/// One row of a picker menu, in pixels.
const MODEL_PICKER_ROW_HEIGHT: f32 = 30.;
/// Rows a model picker shows before it scrolls.
const MODEL_PICKER_VISIBLE_ROWS: usize = 8;

#[derive(Clone)]
struct Toast {
    title: SharedString,
    body: SharedString,
}

#[derive(Clone)]
struct ReaderState {
    title: SharedString,
    markdown: SharedString,
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
    Background,
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

pub(crate) struct DesktopRoot {
    controller: DesktopController,
    snapshot: Arc<DesktopSnapshot>,
    scroller: Entity<MessageScrollerState>,
    prompt: Entity<PromptBar>,
    sidebar_nav: Entity<SidebarNav>,
    extensions_table: Entity<FilterTable>,
    assistant_selections: HashMap<(String, RowId), Entity<SelectionActions>>,
    transcript_rows: HashMap<RowId, Entity<TranscriptRowView>>,
    pending_row_updates: HashSet<RowId>,
    /// Thinking rows the reader expanded inline. The `Thinking` component is
    /// controlled, so the shell owns this state and the trace stays in the
    /// transcript instead of opening a reader.
    expanded_thinking: HashSet<RowId>,
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
    left_visible: bool,
    right_visible: bool,
    overlay: Option<Overlay>,
    overlay_stack: Vec<Overlay>,
    extension_query: String,
    extension_manage: bool,
    slash_overlay: Option<SlashOverlayState>,
    slash_dismissed_for: Option<String>,
    permission_picker_open: bool,
    model_picker_cursor: usize,
    /// Quick filter for the model pickers. A provider's index can hold dozens
    /// of ids, so the list is searchable and navigable rather than scrolled
    /// by hand.
    model_search: Entity<gpui_component::input::InputState>,
    model_picker_query: String,
    /// Scroll owner for the model rows, so keyboard selection can bring the
    /// highlighted row into view.
    model_picker_scroll: ScrollHandle,
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
    recent_commands: Vec<CommandId>,
    sticky_tasks_collapsed: bool,
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
    toast: Option<Toast>,
    focus_restore: crate::ui::shell::FocusRestoration<gpui::FocusHandle>,
    last_surface_revision: Option<String>,
    /// The transcript row whose reader request is in flight.
    pending_reader: Option<ReaderRef>,
    /// Pending decisions currently drawn on the dock/taskbar badge.
    last_badge: usize,
    last_commit_revision: u64,
    last_appearance: Appearance,
    last_language: tact_catalog::CatalogLocale,
    mentions_root: Option<PathBuf>,
    prompt_mentions: Vec<PromptMention>,
    sidebar_query: String,
    rail_search: Entity<gpui_component::input::InputState>,
    _subscriptions: Vec<gpui::Subscription>,
    attachments: Vec<PathBuf>,
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
        let scroller = cx.new(|cx| MessageScrollerState::new(snapshot.transcript.len(), cx));
        let prompt = cx.new(|cx| PromptBar::new("tact-composer", window, cx));
        let root_weak = cx.weak_entity();
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
        let queue_focus = cx.focus_handle();
        let overlay_focus = cx.focus_handle();

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
            |this, _, event: &SidebarNavEvent, _, cx| match event {
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
                SidebarNavEvent::CollapsedChanged { .. } => {}
            },
        );
        let rail_search_subscription = cx.subscribe_in(
            &rail_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, window, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    let query = this.rail_search.read(cx).value().to_string();
                    this.sidebar_query = query.clone();
                    this.sidebar_nav.update(cx, |nav, cx| {
                        nav.set_query(query, window, cx);
                    });
                    cx.notify();
                }
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
                FilterTableEvent::ActivationRequested { row_id, .. } => {
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
            |this, _, event: &gpui_component::input::InputEvent, _, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    let query = this.session_search.read(cx).value().to_string();
                    this.dispatch(DesktopIntent::SetSessionQuery(query));
                }
            },
        );
        let extension_search_subscription = cx.subscribe_in(
            &extension_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, _, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.extension_query = this.extension_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        );
        let model_search_subscription = cx.subscribe_in(
            &model_search,
            window,
            |this, _, event: &gpui_component::input::InputEvent, _, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.model_picker_query = this.model_search.read(cx).value().to_string();
                    // A query change reorders nothing but rebuilds the visible
                    // set, so the old cursor would point at a different model.
                    this.model_picker_cursor = 0;
                    this.model_picker_scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
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
            prompt,
            sidebar_nav,
            extensions_table,
            assistant_selections: HashMap::new(),
            transcript_rows: HashMap::new(),
            pending_row_updates: HashSet::new(),
            expanded_thinking: HashSet::new(),
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
            left_visible: true,
            right_visible: false,
            overlay: None,
            overlay_stack: Vec::new(),
            extension_query: String::new(),
            extension_manage: false,
            slash_overlay: None,
            slash_dismissed_for: None,
            permission_picker_open: false,
            model_picker_cursor: 0,
            model_search,
            model_picker_query: String::new(),
            model_picker_scroll: ScrollHandle::new(),
            overlay_epoch: 0,
            submitted_gates: HashSet::new(),
            #[cfg(test)]
            dispatched_intents: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            platform_prompts: std::sync::atomic::AtomicUsize::new(0),
            queue_cursor: 0,
            queue_expanded: false,
            recent_commands: Vec::new(),
            sticky_tasks_collapsed: true,
            new_activity: false,
            voice,
            pending_voice_text: None,
            history_cursor: None,
            session_project_filter: None,
            toast: None,
            focus_restore: crate::ui::shell::FocusRestoration::default(),
            last_surface_revision: None,
            pending_reader: None,
            last_badge: 0,
            last_commit_revision: initial_revision,
            last_appearance: Appearance::Light,
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
            ],
            attachments: Vec::new(),
        };
        root.last_appearance = root.snapshot.appearance;
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

    fn session_title(&self, session: &tact_desktop_core::SessionSummaryView) -> String {
        session_title_label(
            self.snapshot.language,
            &session.title,
            session.message_count,
        )
    }

    fn sync_sidebar_nav(&self, window: &mut Window, cx: &mut Context<Self>) {
        let today = chrono::Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let mut buckets: [Vec<SidebarNavItem>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for session in &self.snapshot.sessions.items {
            let title = self.session_title(session);
            let locked = session.locked
                && self.snapshot.session.id.as_ref() != Some(&SessionId(session.id.clone()));
            let detail = crate::ui::session_rail::session_detail(
                session,
                chrono::Utc::now(),
                self.snapshot.language,
            );
            let updated = session.updated_at.with_timezone(&chrono::Local);
            let day = updated.date_naive();
            let bucket = if day >= today {
                0
            } else if day == yesterday {
                1
            } else {
                2
            };
            let trailing = if bucket == 0 {
                updated.format("%H:%M").to_string()
            } else {
                updated.format("%b %-d").to_string()
            };
            let item = SidebarNavItem::new(session.id.clone(), title)
                .detail(detail)
                .trailing(trailing)
                .disabled(locked);
            let item = if locked {
                item.badge(self.text(TextKey::Locked).to_string())
            } else {
                item
            };
            buckets[bucket].push(item);
        }
        let (labels, active) = (
            sidebar_nav_labels(self.snapshot.language),
            self.snapshot.session.id.as_ref().map(|id| id.0.clone()),
        );
        let (today_label, yesterday_label, earlier_label) = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => ("TODAY", "YESTERDAY", "EARLIER"),
            tact_catalog::CatalogLocale::Zh => ("今天", "昨天", "更早"),
        };
        let mut sections = Vec::new();
        for (index, label) in [today_label, yesterday_label, earlier_label]
            .into_iter()
            .enumerate()
        {
            if buckets[index].is_empty() {
                continue;
            }
            sections.push(
                SidebarSection::new(
                    SharedString::from(format!("sessions-{index}")),
                    SharedString::from(label),
                )
                .items(std::mem::take(&mut buckets[index])),
            );
        }
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
            }
        });
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
        let open_label = self.text(TextKey::Open);
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
            table.set_rows(Progressive::complete(Arc::from(rows)), cx);
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
            Overlay::Background => "background",
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
                self.pending_row_updates
                    .extend(commit.dirty_rows.iter().copied());
                if commit_gap || epoch_changed {
                    self.pending_row_updates
                        .extend(commit.snapshot.transcript.iter().map(|row| row.id));
                }
                if epoch_changed {
                    self.transcript_rows.clear();
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
                let session_changed = self.snapshot.session.epoch != commit.snapshot.session.epoch;
                self.snapshot = commit.snapshot;
                if session_changed {
                    // Row ids restart per session; an expansion from the old
                    // session must not open a same-numbered row here.
                    self.expanded_thinking.clear();
                }
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
                if appearance_changed || self.snapshot.appearance != self.last_appearance {
                    self.last_appearance = self.snapshot.appearance;
                    theme::apply(self.snapshot.appearance, None, cx);
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
                        self.toast = Some(Toast {
                            title: self.text(TextKey::VoiceInput).into(),
                            body: message.into(),
                        });
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
                self.toast = Some(Toast {
                    title: self.text(TextKey::TactStopped).into(),
                    body: failure.message.to_string().into(),
                });
                cx.notify();
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
                self.toast = Some(Toast {
                    title: title.into(),
                    body: body.into(),
                });
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
                            this.toast = Some(Toast {
                                title: this.text(TextKey::TranscriptSaved).into(),
                                body: path.display().to_string().into(),
                            });
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            DesktopEffect::WindowCloseRequested => cx.quit(),
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
        self.expanded_thinking.insert(row);
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
    pub(crate) fn capture_open_background(&mut self) {
        self.open_overlay(Overlay::Background);
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
    pub(crate) fn capture_set_inspector(&mut self, tab: InspectorTab, cx: &mut Context<Self>) {
        let mut snapshot = (*self.snapshot).clone();
        snapshot.inspector.tab = tab;
        self.snapshot = Arc::new(snapshot);
        self.right_visible = true;
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
                self.toast = Some(Toast {
                    title: self.text(TextKey::PromptEnhancement).into(),
                    body: self.text(TextKey::PromptEnhancementUnavailable).into(),
                });
                cx.notify();
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
            prompt.set_draft(suggestion.insertion.clone(), window, cx);
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
        self.toast = Some(Toast {
            title: self.text(TextKey::UnknownCommand).into(),
            body: text.to_string().into(),
        });
        cx.notify();
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
            self.toast = Some(Toast {
                title: self.text(TextKey::CommandUnavailable).into(),
                body: descriptor.description.get(self.snapshot.language).into(),
            });
            cx.notify();
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
                    self.toast = Some(Toast {
                        title: self.text(TextKey::VoiceInput).into(),
                        body: self.text(TextKey::VoiceInputUnavailable).into(),
                    });
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
        self.model_picker_scroll.set_offset(point(px(0.), px(0.)));
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
    /// Rows are a fixed height, so the visible band is arithmetic rather than
    /// a query to the scroll view. The scroll offset is read back on every
    /// move, which is what lets the pointer and the keyboard share the list:
    /// a wheel scroll is respected, not undone by the next arrow key.
    fn reveal_model_cursor(&self, cx: &mut Context<Self>) {
        let count = self.model_picker_len();
        if count == 0 {
            return;
        }
        let row = px(MODEL_PICKER_ROW_HEIGHT);
        let visible = px(MODEL_PICKER_ROW_HEIGHT * MODEL_PICKER_VISIBLE_ROWS as f32);
        let top = self.model_picker_scroll.offset().y;
        let cursor_top = row * self.model_picker_cursor as f32;
        let cursor_bottom = cursor_top + row;
        let offset = if cursor_top < top {
            Some(cursor_top)
        } else if cursor_bottom > top + visible {
            Some(cursor_bottom - visible)
        } else {
            None
        };
        if let Some(offset) = offset {
            self.model_picker_scroll.set_offset(point(px(0.), offset));
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
                PromptAttachment::new(
                    path.display().to_string(),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(self.text(TextKey::Attachment)),
                )
                .size_bytes(std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
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

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self
            .snapshot
            .project
            .repository
            .clone()
            .unwrap_or_else(|| self.snapshot.project.display_name.clone());
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

    fn render_session_rail(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = crate::ui::shell::geometry(self.snapshot.appearance);
        // The design caps the session list at 32% of the window's height
        // (190–330px) instead of letting it eat the rail's spare space. The
        // DURABLE block below it also needs room at 840px, so the cap is
        // taken slightly lower there.
        let session_list_height = (f32::from(window.viewport_size().height) * 0.20)
            .clamp(190., 330.)
            * self.snapshot.text_scale;
        let project = self
            .snapshot
            .project
            .repository
            .clone()
            .unwrap_or_else(|| self.snapshot.project.display_name.clone());
        let session_count = self.snapshot.sessions.items.len();
        let goal_count = self.snapshot.inspector.goals.len();
        let subagent_count = self.snapshot.inspector.subagents.len();
        let background_count = self.snapshot.inspector.background.len();
        let project_meta = match (
            self.snapshot.project.branch.as_deref(),
            self.snapshot.language,
        ) {
            (Some(branch), tact_catalog::CatalogLocale::En) => {
                format!("{session_count} sessions · {branch}")
            }
            (Some(branch), tact_catalog::CatalogLocale::Zh) => {
                format!("{session_count} 个会话 · {branch}")
            }
            (None, tact_catalog::CatalogLocale::En) => format!("{session_count} sessions"),
            (None, tact_catalog::CatalogLocale::Zh) => format!("{session_count} 个会话"),
        };
        self.sync_sidebar_nav(window, cx);

        v_flex()
            .id("session-rail")
            .debug_selector(|| "session-rail".into())
            .role(Role::Navigation)
            .aria_label(self.text(TextKey::ProjectsAndSessions))
            .w(px(f32::from(geometry.rail_width) * self.snapshot.text_scale))
            .h_full()
            .flex_none()
            .pt(px(14.))
            .px(px(12.))
            .pb(px(12.))
            .gap_1()
            .font_family(theme::native_font_family())
            .border_r(px(theme::border_width()))
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .id("open-project")
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(self.text(TextKey::OpenProject))
                    .cursor_pointer()
                    .items_center()
                    .gap(px(9.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(cx.theme().sidebar_border)
                    .bg(cx.theme().popover)
                    .px(px(8.))
                    .py(px(5.))
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        div()
                            .flex_none()
                            .size(px(29.))
                            .rounded(px(8.))
                            .bg(cx.theme().accent)
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .font_semibold()
                            .text_color(cx.theme().link)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(project_initials(project.as_ref())),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .font_semibold()
                                    .child(project.to_string()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(project_meta.clone()),
                            ),
                    )
                    .when(!self.snapshot.project.available, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .rounded_full()
                                .bg(cx.theme().danger)
                                .px_2()
                                .py_0p5()
                                .text_xs()
                                .text_color(cx.theme().primary_foreground)
                                .child(self.text(TextKey::Unavailable)),
                        )
                    })
                    .child(
                        Icon::new(IconName::ChevronsUpDown)
                            .size(px(13.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.open_project_picker(cx);
                    })),
            )
            .child(
                h_flex()
                    .id("new-run")
                    .debug_selector(|| "new-run".into())
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(self.text(TextKey::NewSession))
                    .cursor_pointer()
                    .w_full()
                    .h(px(37.))
                    .mt(px(8.))
                    .rounded(px(7.))
                    .bg(cx.theme().primary)
                    .text_color(cx.theme().primary_foreground)
                    .px(px(10.))
                    .gap(px(9.))
                    .items_center()
                    .child(Icon::new(IconName::Plus).size(px(15.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .whitespace_nowrap()
                            .text_sm()
                            .font_semibold()
                            .child(self.text(TextKey::NewRun)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .rounded(px(5.))
                            .bg(gpui::rgba(0xffffff33))
                            .px(px(5.))
                            .py(px(1.))
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .child(keymap::new_session_hint()),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.dispatch(DesktopIntent::NewSession);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("rail-search")
                    .debug_selector(|| "rail-search".into())
                    .w_full()
                    .h(px(33.))
                    .mt(px(8.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(cx.theme().sidebar_border)
                    .bg(cx.theme().popover)
                    .px(px(10.))
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(13.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.rail_search)
                                .aria_label(self.text(TextKey::SearchSessions)),
                        ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .rounded(px(5.))
                            .bg(cx.theme().muted)
                            .px(px(5.))
                            .py(px(1.))
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child("/"),
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
                    // The design caps the session list and lets the
                    // extensions block follow it; the rail's spare height
                    // belongs to the quota card at the bottom. `flex_1` lets
                    // a short window shrink the list instead of pushing the
                    // quota card off the rail.
                    .flex_none()
                    .h(px(session_list_height))
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
            .child(
                v_flex()
                    .id("rail-durable")
                    .debug_selector(|| "rail-durable".into())
                    .gap(px(2.))
                    .mt(px(8.))
                    .child(
                        div()
                            .px(px(14.))
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::Durable)),
                    )
                    .child(
                        extension_rail_row(
                            "rail-goals",
                            IconName::CircleCheck,
                            self.text(TextKey::Goals),
                            Some(goal_count.to_string()),
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.select_inspector(InspectorTab::Goals, cx);
                            },
                        )),
                    )
                    .child(
                        extension_rail_row(
                            "rail-subagents",
                            IconName::CircleUser,
                            self.text(TextKey::Subagents),
                            Some(subagent_count.to_string()),
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.select_inspector(InspectorTab::Subagents, cx);
                            },
                        )),
                    )
                    .child(
                        extension_rail_row(
                            "rail-background",
                            IconName::Inbox,
                            self.text(TextKey::Background),
                            Some(background_count.to_string()),
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.open_overlay(Overlay::Background);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                v_flex()
                    .id("rail-extensions")
                    .debug_selector(|| "rail-extensions".into())
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().sidebar_border)
                    .pt(px(14.))
                    .child(
                        div()
                            .px(px(10.))
                            .text_size(px(10.))
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::Extensions).to_uppercase()),
                    )
                    .child(
                        extension_rail_row(
                            "extensions-mcp",
                            IconName::Network,
                            self.text(TextKey::Mcp),
                            self.snapshot
                                .surface
                                .as_ref()
                                .and_then(|surface| match surface {
                                    SurfaceSnapshot::McpServers(servers) => Some(servers.len()),
                                    _ => None,
                                })
                                .map(|count| count.to_string()),
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.open_overlay(Overlay::Extensions(ExtensionsTab::Mcp));
                                this.dispatch(DesktopIntent::InvokeCommand {
                                    id: CommandId::Builtin(BuiltinCommand::Mcp),
                                    args: CommandArguments::default(),
                                });
                            },
                        )),
                    )
                    .child(
                        extension_rail_row(
                            "extensions-skills",
                            IconName::LayoutDashboard,
                            self.text(TextKey::Skills),
                            Some(self.snapshot.extensions.snapshot.skills.len().to_string()),
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.open_overlay(Overlay::Extensions(ExtensionsTab::Skills));
                                this.dispatch(DesktopIntent::InvokeCommand {
                                    id: CommandId::Builtin(BuiltinCommand::Skill),
                                    args: CommandArguments::default(),
                                });
                            },
                        )),
                    )
                    .child(
                        extension_rail_row(
                            "extensions-plugins",
                            IconName::Bot,
                            self.text(TextKey::Plugins),
                            Some(self.snapshot.extensions.snapshot.plugins.len().to_string()),
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.remember_focus(window, cx);
                                this.open_overlay(Overlay::Extensions(ExtensionsTab::Plugins));
                                this.dispatch(DesktopIntent::InvokeCommand {
                                    id: CommandId::Builtin(BuiltinCommand::Plugin),
                                    args: CommandArguments::default(),
                                });
                            },
                        )),
                    ),
            )
            // The design pins the quota card and Settings to the rail's
            // bottom edge; the spare height sits between them and the
            // extension list rather than stretching the session list.
            .child(div().flex_1().min_h(px(8.)))
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
                                            .w(px((f32::from(geometry.rail_width) - 36.0)
                                                * self.snapshot.text_scale
                                                * ratio.clamp(0.0, 1.0)))
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

    fn sync_assistant_selections(
        &mut self,
        transcript: &tact_desktop_core::TranscriptView,
        scope: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashMap<(String, RowId), Entity<SelectionActions>> {
        let mut live = HashSet::new();
        for row in transcript.iter() {
            let RowBody::Assistant(assistant) = &row.body else {
                continue;
            };
            live.insert(row.id);
            let display_markdown = mermaid::strip_blocks(&assistant.markdown);
            let key = (scope.to_string(), row.id);
            if let Some(selection) = self.assistant_selections.get(&key) {
                selection.update(cx, |selection, cx| {
                    selection.set_markdown(display_markdown.clone(), cx);
                });
                continue;
            }
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
                .text_style(TextViewStyle {
                    paragraph_gap: gpui::rems(0.56),
                    heading_font_size: Some(std::sync::Arc::new(|level, _base| match level {
                        1 => px(21.),
                        2 => px(18.),
                        3 => px(16.),
                        _ => px(13.),
                    })),
                    is_dark: self.snapshot.appearance == Appearance::Dark,
                    ..Default::default()
                })
            });
            selection.update(cx, |selection, cx| {
                selection.set_actions(
                    [
                        SelectionAction::new(
                            "ask",
                            i18n::text(self.snapshot.language, TextKey::Ask),
                        ),
                        SelectionAction::new(
                            "explain",
                            i18n::text(self.snapshot.language, TextKey::Explain),
                        ),
                        SelectionAction::new(
                            "rewrite",
                            i18n::text(self.snapshot.language, TextKey::Rewrite),
                        ),
                    ],
                    cx,
                );
            });
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
            self.assistant_selections.insert(key, selection);
        }
        self.assistant_selections
            .retain(|(key_scope, row_id), _| key_scope != scope || live.contains(row_id));
        self.assistant_selections.clone()
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
        let assistant_selections = self.sync_assistant_selections(&transcript, "main", window, cx);
        let row_views = self.sync_transcript_row_views(
            &transcript,
            snapshot.clone(),
            &assistant_selections,
            cx,
        );
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
            .run_started_at
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
        let scroller = MessageScroller::new(
            "desktop-transcript",
            self.scroller.clone(),
            move |index, _, _| {
                row_views
                    .get(index)
                    .cloned()
                    .map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element())
            },
        )
        .scrollbar(true)
        .jump_button(false)
        .p_0()
        .with_row_style(full_width_row_style());
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
                    .gap_2()
                    .px_0()
                    .pt(px(50.))
                    .pb(px(28.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
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
                        // The design's hero is `clamp(24px, 2.25vw, 32px)`;
                        // CJK uses 1.12 line height and an 840px measure.
                        let cjk_title = is_cjk_title(self.snapshot.language, &title);
                        let headline = (f32::from(window.viewport_size().width) * 0.0225)
                            .clamp(24., 32.)
                            * self.snapshot.text_scale;
                        div()
                            .max_w(px(900.))
                            .font_family("SF Pro Display")
                            .font_semibold()
                            .when(cjk_title, |title| {
                                title
                                    .max_w(px(840. * self.snapshot.text_scale))
                                    .text_size(px(headline))
                                    .line_height(px(headline * 1.12))
                            })
                            .when(!cjk_title, |title| {
                                title
                                    .text_size(px(headline))
                                    .line_height(px(headline * 1.24))
                            })
                            .child(title)
                    })
                    .when_some(subtitle, |column, subtitle| {
                        column.child(
                            div()
                                .max_w(px(900.))
                                .text_size(px(14.))
                                .line_height(px(23.))
                                .text_color(cx.theme().muted_foreground)
                                .child(subtitle),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(scroller));
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
        snapshot: Arc<DesktopSnapshot>,
        assistant_selections: &HashMap<(String, RowId), Entity<SelectionActions>>,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<TranscriptRowView>> {
        let live = transcript.iter().map(|row| row.id).collect::<HashSet<_>>();
        self.transcript_rows.retain(|id, _| live.contains(id));
        let mut views = Vec::with_capacity(transcript.len());
        for row in transcript.iter() {
            let selection = assistant_selections
                .get(&("main".to_string(), row.id))
                .cloned();
            let thinking_open = self.expanded_thinking.contains(&row.id);
            let needs_update = self.pending_row_updates.contains(&row.id)
                || !self.transcript_rows.contains_key(&row.id);
            let view = if let Some(view) = self.transcript_rows.get(&row.id).cloned() {
                let context_changed = view.read(cx).snapshot.appearance != snapshot.appearance
                    || view.read(cx).snapshot.language != snapshot.language
                    || view.read(cx).snapshot.show_hook_output != snapshot.show_hook_output
                    || view.read(cx).thinking_open != thinking_open;
                if needs_update || context_changed {
                    view.update(cx, |view, cx| {
                        view.row = row.clone();
                        view.snapshot = snapshot.clone();
                        view.assistant_selection = selection;
                        view.thinking_open = thinking_open;
                        cx.notify();
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
                self.transcript_rows.insert(row.id, view.clone());
                view
            };
            views.push(view);
        }
        self.pending_row_updates.clear();
        views
    }

    fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = crate::ui::shell::geometry(self.snapshot.appearance);
        let goals = self.snapshot.inspector.goals.len();
        let agents = self.snapshot.inspector.subagents.len();
        let tabs = [
            (InspectorTab::Run, self.text(TextKey::Run).to_string()),
            (
                InspectorTab::Goals,
                format!("{} {goals}", self.text(TextKey::Goals)),
            ),
            (
                InspectorTab::Subagents,
                format!("{} {agents}", self.text(TextKey::Subagents)),
            ),
            (
                InspectorTab::Context,
                self.text(TextKey::Context).to_string(),
            ),
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
                .aria_label(label.clone())
                .cursor_pointer()
                .px(px(10.))
                .pt(px(8.))
                .pb(px(7.))
                .text_size(px(12.))
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
        };
        let tab_index = match self.snapshot.inspector.tab {
            InspectorTab::Run => 0,
            InspectorTab::Goals => 1,
            InspectorTab::Subagents => 2,
            InspectorTab::Context => 3,
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
            .w(px(
                f32::from(geometry.inspector_width) * self.snapshot.text_scale
            ))
            .h_full()
            .flex_none()
            .p(px(16.))
            .gap(px(14.))
            .font_family(theme::native_font_family())
            .border_l(px(theme::border_width()))
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
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
                    .when_some(runtime.latest_tool.clone(), |column, tool| {
                        column.child(Self::activity_line(tool.to_string(), true, cx))
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
                            "Ctrl 1-4 · Inspect without losing the run"
                        }
                        tact_catalog::CatalogLocale::Zh => "Ctrl 1-4 · 检查而不离开运行",
                    }),
            )
            .into_any_element()
    }

    fn background_task_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.snapshot
            .inspector
            .background
            .iter()
            .map(|task| {
                let elapsed = task
                    .finished_at
                    .unwrap_or_else(|| chrono::Utc::now().timestamp())
                    .saturating_sub(task.started_at);
                v_flex()
                    .id(SharedString::from(format!(
                        "inspector-background-{}",
                        task.id
                    )))
                    .role(Role::Status)
                    .aria_label(format!(
                        "{} · {} · {}s",
                        background_status_label(task.status, self.snapshot.language),
                        task.command,
                        elapsed
                    ))
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
                            .child(div().text_xs().child(format!(
                                "{} · {elapsed}s",
                                background_status_label(task.status, self.snapshot.language)
                            )))
                            .child(
                                div()
                                    .id(SharedString::from(format!("open-background-{}", task.id)))
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
                                            this.dispatch(DesktopIntent::OpenReader(
                                                ReaderRef::Background(id.clone()),
                                            ));
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
                            .child(format_duration_ms(agent.elapsed_ms)),
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
                RowBody::HookContext(hook) if row.id == id => Some((
                    hook.source
                        .as_deref()
                        .unwrap_or(self.text(TextKey::HookSources))
                        .to_string(),
                    hook.markdown.to_string(),
                )),
                _ => None,
            })
    }

    fn render_gate_banner(
        &self,
        gate: tact_desktop_core::GateCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
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
                    .child(gate.prompt.to_string()),
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
                            if let Some(row) = this.snapshot.transcript.get(index)
                                && let Some(view) = this.transcript_rows.get(&row.id).cloned()
                            {
                                let focus = view.read(cx).gate_focus.clone();
                                focus.focus(window, cx);
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
        let rows = tasks
            .into_iter()
            .enumerate()
            .map(|(index, task)| {
                let marker = match task.status {
                    tact_desktop_core::TaskStatusSnapshot::Pending => div()
                        .flex_none()
                        .size(px(12.))
                        .rounded_full()
                        .border_1()
                        .border_color(cx.theme().muted_foreground)
                        .into_any_element(),
                    tact_desktop_core::TaskStatusSnapshot::InProgress => {
                        Icon::new(IconName::LoaderCircle)
                            .size(px(13.))
                            .text_color(cx.theme().info)
                            .into_any_element()
                    }
                    tact_desktop_core::TaskStatusSnapshot::Completed => {
                        Icon::new(IconName::CircleCheck)
                            .size(px(13.))
                            .text_color(cx.theme().success)
                            .into_any_element()
                    }
                };
                let muted = task.status == tact_desktop_core::TaskStatusSnapshot::Completed;
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
                    .child(marker)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(muted, |text| text.text_color(cx.theme().muted_foreground))
                            .child(task.subject.to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .font_family("Lilex")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{:02}", index + 1)),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let progress = if task_total == 0 {
            0.0
        } else {
            completed as f32 / task_total as f32
        };
        let goals = self.snapshot.inspector.goals.len();
        let agents = self.snapshot.inspector.subagents.len();
        let background = self.snapshot.inspector.background.len();
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
            .when(task_total > 0, |strip| {
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
                                    false,
                                    cx,
                                ))
                                .child(sticky_count(
                                    self.text(TextKey::Subagents),
                                    agents,
                                    InspectorTab::Subagents,
                                    false,
                                    cx,
                                ))
                                .when(background > 0, |header| {
                                    header.child(sticky_count(
                                        self.text(TextKey::Background),
                                        background,
                                        InspectorTab::Run,
                                        true,
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
                            card.child(v_flex().w_full().min_w_0().gap_1().children(rows))
                        }),
                )
            })
            .into_any_element()
    }

    fn render_back_to_bottom(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = format!(
            "{} · {}",
            self.text(TextKey::BackToBottom),
            self.text(TextKey::NewActivity)
        );
        h_flex()
            .id("back-to-bottom-strip")
            .absolute()
            .right_4()
            .bottom_4()
            .occlude()
            .items_center()
            .gap(px(6.))
            .when(self.new_activity, |strip| {
                strip.child(
                    div()
                        .rounded_full()
                        .bg(cx.theme().accent)
                        .text_color(cx.theme().link)
                        .px_3()
                        .py_2()
                        .text_xs()
                        .font_semibold()
                        .child(self.text(TextKey::NewActivity)),
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
                            .child(self.text(TextKey::BackToBottom))
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
                        this.dispatch(DesktopIntent::BackToBottom);
                    })),
            )
            .into_any_element()
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
            .anchor(Anchor::TopLeft)
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
        vec![self.render_permission_picker(cx)]
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
                            // pushing the transcript off screen.
                            .max_w(px(820.))
                            .max_h(px(240.))
                            .mx_auto()
                            .overflow_y_scrollbar()
                            .child(queue_view),
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
                Some(
                    v_flex()
                        .id("command-form-overlay")
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
                        .child(div().font_semibold().child(form.title.clone()))
                        .when_some(form.field.clone(), |panel, field| {
                            panel.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(field),
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
                                        .when(!can_submit, |button| button.opacity(0.45))
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
            Overlay::Background => Some(self.render_background_overlay(window, cx)),
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
        if let Some(Overlay::Reader(stored)) = &mut self.overlay {
            stored.artifact_view = reader.artifact_view;
            stored.diff_table = reader.diff_table.take();
            stored.decisions = std::mem::take(&mut reader.decisions);
            stored.diff_epoch = reader.diff_epoch;
            stored.diff_applied = reader.diff_applied;
        }
        let title = reader.title.clone();
        Some(
            v_flex()
                .id("reader-overlay")
                .role(Role::Dialog)
                .aria_label(title.clone())
                .w(overlay_width(window, 760.))
                .max_h(overlay_height(window, 640.))
                .p_4()
                .gap_3()
                .overflow_y_scroll()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .shadow_lg()
                .child(
                    h_flex()
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
                .child(body)
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
            return InsightCard::new("balance-reader-card", reader.title.to_string())
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
                            StreamedContent::done(text.to_string()),
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
                TextView::markdown("reader-markdown", reader.markdown.to_string())
                    .into_any_element()
            }
            Some(ReaderRef::Diff(row)) => {
                if let Some(patch) = self.diff_row_patch(row).cloned()
                    && diff_review::is_multi_file(&patch)
                {
                    let files = diff_review::files(&patch);
                    return self.diff_table_body(reader, &files, window, cx);
                }
                TextView::markdown("reader-markdown", reader.markdown.to_string())
                    .into_any_element()
            }
            _ => TextView::markdown("reader-markdown", reader.markdown.to_string())
                .into_any_element(),
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
                table.set_columns(
                    [
                        DiffColumn::new("file", "File").width_in_rems(20.),
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
        } else if reader.diff_epoch != reader.diff_applied {
            if let Some(table) = &reader.diff_table {
                table.update(cx, |table, cx| {
                    table.set_rows(diff_rows(files, &reader.decisions), window, cx);
                });
            }
            reader.diff_applied = reader.diff_epoch;
        }
        div()
            .h(px(260.))
            .w_full()
            .debug_selector(|| "reader-diff-table".into())
            .child(reader.diff_table.clone().unwrap())
            .into_any_element()
    }

    fn render_background_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tasks = self.background_task_rows(cx);
        v_flex()
            .id("background-overlay")
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::Background))
            .w(overlay_width(window, 560.))
            .max_h(px(520.))
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().font_semibold().child(self.text(TextKey::Background)))
                    .child(
                        div()
                            .id("close-background")
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
                v_flex()
                    .id("background-task-list")
                    .gap_2()
                    .overflow_y_scroll()
                    .when(tasks.is_empty(), |list| {
                        list.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.text(TextKey::NoItems)),
                        )
                    })
                    .children(tasks),
            )
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
        let rows = models
            .iter()
            .enumerate()
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
                .selected(chosen)
                .cursor_pointer()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(if active {
                    cx.theme().primary
                } else {
                    cx.theme().border
                })
                .px_2()
                // A fixed row height is what lets the keyboard keep its
                // cursor on screen without measuring: the visible band is
                // arithmetic.
                .h(px(MODEL_PICKER_ROW_HEIGHT - 2.))
                .py_0()
                .when(chosen, |row| row.bg(cx.theme().accent))
                .child(div().w_full().min_w_0().truncate().child(value.clone()))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.model_picker_cursor = index;
                    this.activate_model_picker(window, cx);
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let list_height =
            MODEL_PICKER_ROW_HEIGHT * rows.len().min(MODEL_PICKER_VISIBLE_ROWS) as f32;
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
                    .child(div().font_semibold().child(title))
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
            .child(Input::new(&self.model_search))
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
                            .overflow_y_scroll()
                            .track_scroll(&self.model_picker_scroll)
                            .child(v_flex().w_full().gap(px(2.)).children(rows)),
                    )
                    .child(gpui_component::scroll::Scrollbar::vertical(
                        &self.model_picker_scroll,
                    )),
            )
            .into_any_element()
    }

    fn render_subagent_overlay(
        &mut self,
        child_id: String,
        _window: &mut Window,
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
        let rows = transcript
            .as_ref()
            .map(|transcript| {
                transcript
                    .iter()
                    .map(|row| {
                        transcript_row_element(
                            row,
                            &self.snapshot,
                            weak.clone(),
                            None,
                            true,
                            None,
                            false,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    div()
                        .text_sm()
                        .child(self.text(TextKey::NoLiveTranscript))
                        .into_any_element(),
                ]
            });
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
            .child(
                v_flex()
                    .id("subagent-transcript")
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .p(px(20.))
                    .overflow_y_scroll()
                    .children(rows),
            )
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
                    .role(Role::Button)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_label(label)
                    .cursor_pointer()
                    .rounded_md()
                    .px_2()
                    .py_2()
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                v_flex()
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
                                v_flex()
                                    .items_end()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .child(setting_display_value(setting, &self.snapshot)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
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
            .max_h(px(680.))
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().font_semibold().child(self.text(TextKey::Settings)))
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
        let height = (f32::from(window.viewport_size().height) - 100.0).clamp(320.0, 620.0);
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
                h_flex()
                    .id(SharedString::from(format!("hook-source-{source}")))
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
                    .max_h(px(320.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .overflow_y_scrollbar()
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .p(px(16.))
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(match self.snapshot.language {
                                    tact_catalog::CatalogLocale::En => "No hooks configured.",
                                    tact_catalog::CatalogLocale::Zh => "未配置钩子。",
                                }),
                        )
                    })
                    .children(rows),
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

        let manager_height = (f32::from(window.viewport_size().height) - 100.0).clamp(320.0, 620.0);
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
                        row.child(div().w(px(360.)).child(Input::new(&self.extension_search)))
                            .child(div().flex_1())
                    })
                    .when(!self.extension_manage, |row| {
                        row.child(div().flex_1().child(Input::new(&self.extension_search)))
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
                                self.text(TextKey::ChangesApply).to_string()
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
        let rows = filtered
            .into_iter()
            .map(|session| {
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
                let subtitle = format!(
                    "{} · {} {}",
                    self.snapshot.project.branch.as_deref().unwrap_or("—"),
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
                h_flex()
                    .id(SharedString::from(format!(
                        "manager-session-{}",
                        session.id
                    )))
                    .debug_selector({
                        let selector = format!("manager-session-{}", session.id);
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
                    .child(div().flex_none().size(px(6.)).rounded_full().bg(dot))
                    .child(
                        v_flex()
                            .id(SharedString::from(format!("resume-session-{}", session.id)))
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
                                        this.dispatch(DesktopIntent::ResumeSession {
                                            session_id: resume_id.clone(),
                                        });
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
                                                .text_color(cx.theme().muted_foreground)
                                                .child(self.text(TextKey::Archived)),
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
                    .child(archive_button)
            })
            .collect::<Vec<_>>();
        let all_projects_label = match self.snapshot.language {
            tact_catalog::CatalogLocale::En => "All projects",
            tact_catalog::CatalogLocale::Zh => "全部项目",
        };
        let all_projects_selected = self.session_project_filter.is_none();
        let mut project_rows = vec![
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
                })),
        ];
        project_rows.extend(self.snapshot.projects.iter().map(|project| {
            let selected = project.id == self.session_project_filter;
            let root = project.root.clone();
            let project_id = project.id.clone();
            // Only the active project's sessions are loaded; showing a count
            // for any other project would render a false zero.
            let count = project.id.as_ref().and_then(|id| {
                crate::ui::session_manager::project_session_count(
                    id,
                    self.snapshot.project.id.as_ref(),
                    &self.snapshot.sessions.items,
                )
            });
            let initials = initials_from_name(&project.display_name);
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
            gpui_base::Button::new(SharedString::from(format!(
                "manager-project-{}",
                project
                    .id
                    .as_ref()
                    .map(|id| id.0.as_str())
                    .unwrap_or("none")
            )))
            .selected(selected)
            .aria_label(project.display_name.to_string())
            .w_full()
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
                                    .child(project.display_name.to_string()),
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
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.session_project_filter = project_id.clone();
                if this.snapshot.project.id != project_id
                    && let Some(root) = root.clone()
                {
                    this.dispatch(DesktopIntent::OpenProject { root });
                    this.close_overlay();
                    this.restore_focus(window, cx);
                }
            }))
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
                })),
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
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(Input::new(&self.session_search)),
                                    )
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
                                    self.snapshot.project.display_name.to_string()
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
        let rail_edge = f32::from(crate::ui::shell::geometry(self.snapshot.appearance).rail_width)
            * self.snapshot.text_scale;
        // Overlays arrive with one restrained fade and rise rather than
        // appearing between two frames. Reduced motion resolves to the settled
        // frame, as every other transition here does.
        let overlay_epoch = self.overlay_epoch;
        let overlay_direct_motion = crate::ui::shell::motion_policy(self.snapshot.motion)
            .direct_state_changes
            || cx.reduce_motion();
        let mut left_panel = self
            .left_visible
            .then(|| self.render_session_rail(window, cx).into_any_element());
        let inline_left = if shell_layout.rail_inline {
            left_panel.take()
        } else {
            None
        };
        let scrim_left = (!shell_layout.rail_inline).then_some(left_panel).flatten();
        let mut right_panel = self
            .right_visible
            .then(|| self.render_inspector(cx).into_any_element());
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
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when_some(inline_left, |row, left| row.child(left))
                    .child(
                        v_flex().id("run-room").flex_1().min_w_0().h_full().child(
                            h_flex()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .relative()
                                                .child(self.render_transcript(window, cx))
                                                .when(
                                                    self.scroller.read(cx).is_scrolled_up()
                                                        || !self.snapshot.tail_following,
                                                    |region| {
                                                        region.child(self.render_back_to_bottom(cx))
                                                    },
                                                ),
                                        )
                                        .when(self.snapshot.failure.is_some(), |column| {
                                            column.child(self.render_failure(cx))
                                        })
                                        .child(self.render_sticky(cx))
                                        .child(self.render_composer(window, cx)),
                                )
                                .when_some(inline_right, |row, right| row.child(right)),
                        ),
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
                                        .w(px(crate::ui::layout::INSPECTOR_WIDTH
                                            * self.snapshot.text_scale))
                                        .h_full()
                                        .border_l_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().sidebar)
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
                                        .w(px(crate::ui::layout::RAIL_WIDTH
                                            * self.snapshot.text_scale))
                                        .h_full()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().sidebar)
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
                            scrim
                                .items_start()
                                .justify_end()
                                .pl(px(rail_edge))
                                .pb(px(12.))
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

fn extension_rail_row(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    count: Option<String>,
    cx: &mut Context<DesktopRoot>,
) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id)
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
        .aria_label(label)
        .cursor_pointer()
        .items_center()
        .gap(px(9.))
        .rounded(px(6.))
        .h(px(29.))
        .px(px(10.))
        .hover(|style| style.bg(cx.theme().muted))
        .child(
            Icon::new(icon)
                .size(px(14.))
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().flex_1().text_xs().child(label))
        .when_some(count, |row, count| {
            row.child(
                div()
                    .text_size(px(10.))
                    .font_family("Lilex")
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
        })
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
    opens_background: bool,
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
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if opens_background {
                this.remember_focus(window, cx);
                this.open_overlay(Overlay::Background);
            } else {
                this.select_inspector(tab, cx);
            }
            cx.notify();
        }))
}

fn sticky_count(
    label: &'static str,
    count: usize,
    tab: InspectorTab,
    opens_background: bool,
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
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if opens_background {
                this.remember_focus(window, cx);
                this.open_overlay(Overlay::Background);
            } else {
                this.select_inspector(tab, cx);
            }
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

fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    }
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

fn run_phase_label(
    phase: tact_desktop_core::RunPhase,
    locale: tact_catalog::CatalogLocale,
) -> &'static str {
    use tact_desktop_core::RunPhase as P;
    match phase {
        P::Idle => i18n::text(locale, TextKey::Ready),
        P::Waiting => i18n::text(locale, TextKey::Queue),
        P::Thinking => i18n::text(locale, TextKey::Thinking),
        P::Streaming => i18n::text(locale, TextKey::Running),
        P::RunningTools => i18n::text(locale, TextKey::Tool),
        P::WaitingForGate => i18n::text(locale, TextKey::PermissionOrQuestionGate),
        P::Cancelling => i18n::text(locale, TextKey::Stopping),
        P::Complete => i18n::text(locale, TextKey::Completed),
        P::Failed => i18n::text(locale, TextKey::Failed),
    }
}

fn goal_status_label(
    status: tact_desktop_core::GoalStatusSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> &'static str {
    use tact_desktop_core::GoalStatusSnapshot as S;
    match status {
        S::Draft => i18n::text(locale, TextKey::Queue),
        S::Queued => i18n::text(locale, TextKey::Queue),
        S::Running => i18n::text(locale, TextKey::Running),
        S::Paused => i18n::text(locale, TextKey::Pause),
        S::WaitingUser | S::WaitingPermission => {
            i18n::text(locale, TextKey::PermissionOrQuestionGate)
        }
        S::Verifying => i18n::text(locale, TextKey::Review),
        S::Succeeded => i18n::text(locale, TextKey::Completed),
        S::Failed => i18n::text(locale, TextKey::Failed),
        S::Cancelled => i18n::text(locale, TextKey::Cancelled),
    }
}

fn goal_phase_label(
    phase: tact_desktop_core::GoalPhaseSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> &'static str {
    match phase {
        tact_desktop_core::GoalPhaseSnapshot::Planning => i18n::text(locale, TextKey::Thinking),
        tact_desktop_core::GoalPhaseSnapshot::Execution => i18n::text(locale, TextKey::Running),
        tact_desktop_core::GoalPhaseSnapshot::Verification => i18n::text(locale, TextKey::Review),
    }
}

fn subagent_status_label(
    status: tact_desktop_core::SubagentStatusSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> &'static str {
    match status {
        tact_desktop_core::SubagentStatusSnapshot::Running => i18n::text(locale, TextKey::Running),
        tact_desktop_core::SubagentStatusSnapshot::Completed => {
            i18n::text(locale, TextKey::Completed)
        }
        tact_desktop_core::SubagentStatusSnapshot::Failed => i18n::text(locale, TextKey::Failed),
        tact_desktop_core::SubagentStatusSnapshot::Cancelled => {
            i18n::text(locale, TextKey::Cancelled)
        }
    }
}

fn localized_wait_reason(reason: &str, locale: tact_catalog::CatalogLocale) -> String {
    match (reason, locale) {
        ("paused by user", tact_catalog::CatalogLocale::Zh) => "用户已暂停".into(),
        ("waiting for user", tact_catalog::CatalogLocale::Zh) => "等待用户".into(),
        ("waiting for permission", tact_catalog::CatalogLocale::Zh) => "等待权限确认".into(),
        ("run interrupted: lease holder is gone", tact_catalog::CatalogLocale::Zh) => {
            "运行中断：租约持有者已消失".into()
        }
        ("lease holder is gone", tact_catalog::CatalogLocale::Zh) => "租约持有者已消失".into(),
        ("needs acceptance", tact_catalog::CatalogLocale::Zh) => "待验收".into(),
        ("needs explicit acceptance", tact_catalog::CatalogLocale::Zh) => "待明确验收".into(),
        ("review", tact_catalog::CatalogLocale::Zh) => "待复核".into(),
        _ => reason.to_string(),
    }
}

fn prompt_bar_labels(locale: tact_catalog::CatalogLocale) -> PromptBarLabels {
    match locale {
        tact_catalog::CatalogLocale::En => PromptBarLabels {
            placeholder: "Message the agent; use @ mentions or / commands".into(),
            composer: "Prompt composer".into(),
            model_prefix: "Model: ".into(),
            no_models: "No models available".into(),
            available_models: "Available models".into(),
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
                    format!("{} {}", tool.name, tool.arg_summary)
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
        },
    }
}

fn background_status_label(
    status: tact_desktop_core::BackgroundTaskStatusSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> &'static str {
    match status {
        tact_desktop_core::BackgroundTaskStatusSnapshot::Running => {
            i18n::text(locale, TextKey::Running)
        }
        tact_desktop_core::BackgroundTaskStatusSnapshot::Completed => {
            i18n::text(locale, TextKey::Completed)
        }
        tact_desktop_core::BackgroundTaskStatusSnapshot::Error => {
            i18n::text(locale, TextKey::Failed)
        }
    }
}

fn mcp_status_label(
    status: &tact_desktop_core::McpServerStatusSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> String {
    match status {
        tact_desktop_core::McpServerStatusSnapshot::Connected { tools } => {
            format!("{} ({tools})", i18n::text(locale, TextKey::Ready))
        }
        tact_desktop_core::McpServerStatusSnapshot::NeedsAuthorization => {
            i18n::text(locale, TextKey::Authorize).to_string()
        }
        tact_desktop_core::McpServerStatusSnapshot::NotConnected => {
            i18n::text(locale, TextKey::Disconnected).to_string()
        }
        tact_desktop_core::McpServerStatusSnapshot::Disabled => {
            i18n::text(locale, TextKey::Off).to_string()
        }
    }
}

fn section_label(label: &'static str, cx: &mut Context<DesktopRoot>) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(label)
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
            .items_start()
            .gap_3()
            .py(px(18.))
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
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap(px(7.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_semibold()
                                    .text_color(snapshot_ink(snapshot))
                                    .child(i18n::text(snapshot.language, TextKey::You)),
                            )
                            .when_some(row.time.clone(), |line, time| {
                                line.child(
                                    div()
                                        .text_size(px(10.))
                                        .font_family("Lilex")
                                        .text_color(snapshot_row_muted(snapshot))
                                        .child(time.to_string()),
                                )
                            }),
                    )
                    .child(div().text_sm().child(TextView::markdown(
                        format!("user-{id}"),
                        user.text.to_string(),
                    )))
                    .when(!user.attachments.is_empty(), |row| {
                        row.child(
                            AttachmentStrip::new(format!("user-attachments-{id}"))
                                .label(i18n::text(snapshot.language, TextKey::Attachment))
                                .items(user.attachments.iter().map(|path| {
                                    PromptAttachment::new(
                                        path.display().to_string(),
                                        path.file_name().and_then(|name| name.to_str()).unwrap_or(
                                            i18n::text(snapshot.language, TextKey::Attachment),
                                        ),
                                    )
                                    .size_bytes(
                                        std::fs::metadata(path)
                                            .map(|metadata| metadata.len())
                                            .unwrap_or(0),
                                    )
                                })),
                        )
                    }),
            )
            .into_any_element(),
        RowBody::Assistant(assistant) => {
            let display_markdown = mermaid::strip_blocks(&assistant.markdown);
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
            // A one-step plan whose only step is the tool it describes would
            // draw the same command twice: once in the plan card and once in
            // the tool card below it. The tool card carries the status,
            // output and disclosure, so the plan yields to it.
            if plan_is_a_single_tool_call(row, plan, &snapshot.transcript) {
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
            let mut trace = ThinkingTrace::new().prose(thinking.markdown.to_string());
            if let Some(duration_ms) = thinking.duration_ms {
                trace = trace.thought_for(Duration::from_millis(duration_ms));
            }
            let progressive = progressive(trace, thinking.status, snapshot.language);
            let reader_weak = weak.clone();
            v_flex()
                .id(SharedString::from(format!("row-thinking-{id}")))
                .debug_selector(move || format!("transcript-row-thinking-{id}"))
                .child(
                    Thinking::new(format!("thinking-{id}"), &progressive)
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
                            let _ = reader_weak.update(cx, |this, cx| {
                                if open {
                                    this.expanded_thinking.insert(row_id);
                                } else {
                                    this.expanded_thinking.remove(&row_id);
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
            div()
                .debug_selector(move || format!("transcript-row-tool-group-{id}"))
                // The design makes the group the card: one border, one
                // radius, and the calls as rows inside it.
                .w_full()
                .min_w_0()
                .rounded(px(12.))
                .border_1()
                .border_color(snapshot_row_border(snapshot))
                .bg(snapshot_row_surface(snapshot))
                .shadow_sm()
                .overflow_hidden()
                .child(
                    ToolGroup::new(format!("tool-group-{id}"))
                        .title(tool_group_title(group, snapshot.language))
                        .count(tool_count)
                        .compact()
                        .active(matches!(
                            group.status,
                            RowStatus::Running | RowStatus::Pending
                        ))
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
        RowBody::HookContext(context) => v_flex()
            .id(SharedString::from(format!("row-hook-context-{id}")))
            .role(Role::Note)
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(snapshot_row_border(snapshot))
            .bg(snapshot_row_background(snapshot))
            .px_3()
            .py_2()
            .text_xs()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(snapshot_row_muted(snapshot))
                    .child(
                        context
                            .source
                            .as_deref()
                            .unwrap_or(i18n::text(snapshot.language, TextKey::Hooks))
                            .to_string(),
                    ),
            )
            .child(TextView::markdown(
                format!("hook-context-{id}"),
                context.markdown.to_string(),
            ))
            .into_any_element(),
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

/// The command a gate prompt quotes in backticks, when the group has no tool
/// to borrow it from.
fn gate_prompt_command(prompt: &str) -> Option<SharedString> {
    let start = prompt.find('`')?;
    let rest = &prompt[start + 1..];
    let end = rest.find('`')?;
    (!rest[..end].trim().is_empty()).then(|| SharedString::from(rest[..end].trim()))
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

fn tool_row_element(
    tool: &ToolRow,
    group_id: RowId,
    locale: tact_catalog::CatalogLocale,
    weak: WeakEntity<DesktopRoot>,
) -> AnyElement {
    let invocation = ToolInvocation::new(tool.tool_id.to_string(), tool.name.to_string())
        .summary(tool.arg_summary.to_string())
        .input(tool.arg_full.as_deref().unwrap_or(&tool.arg_summary))
        .output(tool_body_markdown(tool, locale))
        // Gate decisions are rendered as first-class gate cards; this inline
        // tool surface must not imply a second, disconnected approval action.
        .approval(ToolApproval::NotRequired);
    let invocation = progressive(invocation, tool.status, locale);
    let call_weak = weak.clone();
    let reader_weak = weak;
    let tool_id = tool.id;
    let mut row = v_flex().gap_1().child(
        ToolCall::new(&invocation)
            .labels(tool_call_labels(locale))
            .grouped()
            .open(tool.expanded)
            .on_event(move |event, _, cx| {
                if matches!(event, ToolCallEvent::Toggled { .. }) {
                    let _ = call_weak.update(cx, |this, _| {
                        this.dispatch(DesktopIntent::ToggleTool {
                            group: group_id,
                            tool: tool_id,
                        });
                    });
                }
            }),
    );
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

/// Builds the `DiffTable` rows for a multi-file patch.
fn diff_rows(
    files: &[diff_review::FileDiff],
    decisions: &[DiffProposalState],
) -> Progressive<Arc<[DiffRow]>> {
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
    Progressive::complete(rows)
}

fn tool_body_markdown(tool: &ToolRow, locale: tact_catalog::CatalogLocale) -> String {
    match &tool.body {
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
    }
}

fn progress_state(status: RowStatus, locale: tact_catalog::CatalogLocale) -> ProgressState {
    match status {
        RowStatus::Pending => ProgressState::Pending,
        RowStatus::Running => ProgressState::Running,
        RowStatus::Failed => ProgressState::Failed(i18n::text(locale, TextKey::ToolFailed).into()),
        RowStatus::Success | RowStatus::Resolved => ProgressState::Complete,
        RowStatus::Cancelled => {
            ProgressState::Failed(i18n::text(locale, TextKey::Cancelled).into())
        }
    }
}

fn progressive<T>(
    content: T,
    status: RowStatus,
    locale: tact_catalog::CatalogLocale,
) -> Progressive<T> {
    match progress_state(status, locale) {
        ProgressState::Pending => Progressive::pending(content),
        ProgressState::Running => Progressive::running(content),
        ProgressState::Complete => Progressive::complete(content),
        ProgressState::Failed(reason) => Progressive::failed(content, reason),
    }
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
        RowStatus::Running => {
            content = StreamedContent::running(markdown.to_string());
        }
        RowStatus::Failed | RowStatus::Cancelled => {
            content = StreamedContent::failed(
                markdown.to_string(),
                i18n::text(locale, TextKey::ResponseFailed),
            );
        }
        RowStatus::Success | RowStatus::Resolved => {}
    }
    content
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
                gate.prompt,
                i18n::text(locale, TextKey::Cancelled)
            )
        } else if let Some(ref settled) = settled {
            format!("{} · {settled}", gate.prompt)
        } else {
            gate.prompt.to_string()
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
                    .child(gate.prompt.to_string()),
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
                .border_color(gpui::rgb(0x5f6673))
                .child(div().text_sm().child(gate.prompt.to_string()))
                .child(
                    div()
                        .text_xs()
                        .text_color(gpui::rgb(0x98a2b3))
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
                                gpui::Hsla::from(gpui::rgb(0xffffff))
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
                    .text_color(gpui::Hsla::from(gpui::rgb(0xffffff)))
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
        let command_description = prompt_command.as_ref().map(|_| match locale {
            tact_catalog::CatalogLocale::En => {
                "This command runs in the current worktree and may write files."
            }
            tact_catalog::CatalogLocale::Zh => "该命令会在当前工作树中运行，并可能写入文件。",
        });
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
                    .child(gate.prompt.to_string()),
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
                card.child(
                    div()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(snapshot_row_border(snapshot))
                        .bg(snapshot_row_background(snapshot))
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(px(12.))
                        .font_family("Lilex")
                        .text_color(snapshot_ink(snapshot))
                        .child(command),
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
                                    .text_color(gpui::Hsla::from(gpui::rgb(0xffffff)))
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
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0xececf1).into(),
        Appearance::Dark => gpui::rgb(0x2c2c2e).into(),
    }
}

fn snapshot_row_border(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgba(0x3c3c4324).into(),
        Appearance::Dark => gpui::rgba(0xffffff1a).into(),
    }
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

fn full_width_row_style() -> gpui::StyleRefinement {
    let mut style = gpui::StyleRefinement::default();
    style.padding.left = Some(px(0.).into());
    style.padding.right = Some(px(0.).into());
    style
}

/// Whether a plan row would only repeat the single tool call it describes.
///
/// The runtime emits a one-step plan whose step is the tool it is about to
/// run. Drawing the plan card and that tool's card then shows the same command
/// twice; the tool card is the richer surface, so the plan step yields to it.
fn plan_is_a_single_tool_call(
    row: &TranscriptRow,
    plan: &tact_desktop_core::PlanRow,
    transcript: &TranscriptView,
) -> bool {
    let [step] = plan.steps.as_ref() else {
        return false;
    };
    let Some(tool_name) = step.detail.as_deref() else {
        return false;
    };
    let rows: Vec<&TranscriptRow> = transcript.iter().map(|row| row.as_ref()).collect();
    let Some(index) = rows.iter().position(|candidate| candidate.id == row.id) else {
        return false;
    };
    // One run owns one execution group, and a later turn in that run gets a
    // fresh loop id while appending to the same group. Compare within the run
    // (the rows between the user turns that surround the plan) rather than by
    // loop id, so the group still absorbs its plan.
    let start = rows[..index]
        .iter()
        .rposition(|candidate| matches!(&candidate.body, RowBody::User(_)))
        .map(|position| position + 1)
        .unwrap_or(0);
    let end = rows[index..]
        .iter()
        .position(|candidate| matches!(&candidate.body, RowBody::User(_)))
        .map(|offset| index + offset)
        .unwrap_or(rows.len());
    rows[start..end]
        .iter()
        .any(|candidate| match &candidate.body {
            RowBody::ToolGroup(group) => group
                .items
                .iter()
                .any(|item| matches!(item, ToolGroupItem::Tool(tool) if &*tool.name == tool_name)),
            _ => false,
        })
}

/// The rail's quick-filter placeholder, short enough for the 250px rail in
/// both locales.
fn rail_search_placeholder(locale: tact_catalog::CatalogLocale) -> &'static str {
    match locale {
        tact_catalog::CatalogLocale::En => "Find a session",
        tact_catalog::CatalogLocale::Zh => "查找会话",
    }
}

/// Whether a row's content starts at the message column rather than at the
/// transcript's left edge.
///
/// The design's row grid gives every row a 31px avatar column and a 12px gap;
/// user and assistant rows draw a mark there, while plan, thinking, tool,
/// diff, gate and notice rows are content that belongs beside the message.
fn row_starts_at_message_column(body: &RowBody) -> bool {
    !matches!(body, RowBody::User(_) | RowBody::Assistant(_))
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
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0xffffff).into(),
        Appearance::Dark => gpui::rgb(0x2c2c2e).into(),
    }
}

/// The design's `--ink`: message labels and the user's avatar fill.
fn snapshot_ink(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0x1d1d1f).into(),
        Appearance::Dark => gpui::rgb(0xf5f5f7).into(),
    }
}

/// The design's `--canvas`: the transcript background, used as the user
/// avatar's ink so the mark reads as a solid stamp in both appearances.
fn snapshot_canvas(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0xf5f5f7).into(),
        Appearance::Dark => gpui::rgb(0x1c1c1e).into(),
    }
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
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0x007aff).into(),
        Appearance::Dark => gpui::rgb(0x0a84ff).into(),
    }
}

fn snapshot_theme_success(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0x34c759).into(),
        Appearance::Dark => gpui::rgb(0x30d158).into(),
    }
}

fn snapshot_theme_danger(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0xff453a).into(),
        Appearance::Dark => gpui::rgb(0xff453a).into(),
    }
}

fn snapshot_theme_warning(_snapshot: &DesktopSnapshot) -> gpui::Hsla {
    gpui::rgb(0xff9f0a).into()
}

fn snapshot_row_muted(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0x6e6e73).into(),
        Appearance::Dark => gpui::rgb(0x98989d).into(),
    }
}

fn snapshot_accent(snapshot: &DesktopSnapshot) -> gpui::Hsla {
    match snapshot.appearance {
        Appearance::Light => gpui::rgb(0x007aff).into(),
        Appearance::Dark => gpui::rgb(0x0a84ff).into(),
    }
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
                this.toast = Some(Toast {
                    title: this.text(TextKey::VoiceInput).into(),
                    body: this.text(TextKey::VoiceInputUnavailable).into(),
                });
                cx.notify();
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
        InspectorTab::Context => InspectorTab::Run,
    }
}

fn previous_inspector(tab: InspectorTab) -> InspectorTab {
    match tab {
        InspectorTab::Run => InspectorTab::Context,
        InspectorTab::Goals => InspectorTab::Run,
        InspectorTab::Subagents => InspectorTab::Goals,
        InspectorTab::Context => InspectorTab::Subagents,
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
    use tact_desktop_core::{DesktopConfig, SessionEpoch, StateCommit};

    use super::*;

    fn install_test_config(config: &DesktopConfig) {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| config.install_process_config().unwrap());
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
            gpui_ai::init(cx);
            crate::theme::apply(Appearance::Light, Some(window), cx);
            DesktopRoot::new_for_test(controller, window, cx)
        });
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        (root, window, shutdown, data_dir)
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
            gpui_ai::init(cx);
            crate::theme::apply(Appearance::Light, Some(window), cx);
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
            plan_is_a_single_tool_call(&plan_row, plan_body, &same_run),
            "a later turn's plan belongs to the run's existing group"
        );
        let next_run = TranscriptView::from_rows(vec![group, user, Arc::new(plan_row.clone())]);
        assert!(
            !plan_is_a_single_tool_call(&plan_row, plan_body, &next_run),
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
        assert_eq!(rows.content().len(), 2);
        assert_eq!(
            rows.content()[0].proposal_state(),
            DiffProposalState::Accepted
        );
        assert_eq!(
            rows.content()[1].proposal_state(),
            DiffProposalState::Pending
        );
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

    #[gpui::test]
    fn thinking_expands_inline_without_a_reader(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
                this.snapshot = Arc::new(snapshot);
                this.transcript_rows.clear();
                let count = this.snapshot.transcript.len();
                this.scroller.update(cx, |state, cx| {
                    state.reset(count, cx);
                    state.scroll_to_item(2, cx);
                });
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
                root.expanded_thinking.contains(&RowId::new(2)),
                "the thought expands in place"
            );
            assert!(
                root.overlay.is_none(),
                "expanding a thought must not open a reader"
            );
        });
        window.simulate_click(toggle, gpui::Modifiers::default());
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        window.update(|_, cx| {
            assert!(
                !root.read(cx).expanded_thinking.contains(&RowId::new(2)),
                "a second click collapses the thought"
            );
        });
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
            gpui_ai::init(cx);
            crate::theme::apply(Appearance::Light, Some(window), cx);
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
        assert!(
            window.debug_bounds("permission-picker-menu").is_some(),
            "clicking the permission trigger must open the anchored menu"
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
            let view = root.transcript_rows.get(&row.id).expect("row view");
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
            gpui_ai::init(cx);
            crate::theme::apply(Appearance::Light, Some(window), cx);
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
            gpui_ai::init(cx);
            crate::theme::apply(Appearance::Light, Some(window), cx);
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
        let extensions = window
            .debug_bounds("rail-extensions")
            .expect("rail extensions bounds");
        let list_bottom = list.origin.y + list.size.height;
        assert!(
            view_all.origin.y - list_bottom <= px(12.),
            "session list ends at {list_bottom:?} but 'view all sessions' starts at {:?}; \
             an empty flex spacer is stealing the rail's free height",
            view_all.origin.y
        );
        assert!(
            view_all.origin.y + view_all.size.height <= extensions.origin.y + px(12.),
            "view-all row overlaps the extensions section"
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
    fn back_to_bottom_floats_above_the_sticky_strip(cx: &mut TestAppContext) {
        let (root, window, shutdown, data_dir) = probe_root(cx);
        window.update(|window, cx| {
            root.update(cx, |this, cx| {
                let mut snapshot = (*this.snapshot).clone();
                snapshot.tail_following = false;
                snapshot.session.phase = SessionPhase::Ready;
                snapshot.session.id = Some(SessionId("back-to-bottom-session".into()));
                snapshot.project.id = Some(ProjectId("back-to-bottom-project".into()));
                this.snapshot = Arc::new(snapshot);
                // The pill is anchored to the transcript, not the window: with
                // the inspector open it must not float over the inspector.
                this.right_visible = true;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
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
        assert_eq!(manager.size.height, px(620.), "manager height");

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
        assert_eq!(hooks.size.height, px(620.), "hooks sheet height");

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
    fn a_one_step_tool_plan_yields_to_the_tool_card() {
        let base = tact_desktop_core::fixtures::demo_transcript();
        let rows = base
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                if matches!(row.body, RowBody::Plan(_)) {
                    row.body = RowBody::Plan(tact_desktop_core::PlanRow {
                        title: Arc::from("Execution plan"),
                        steps: Arc::from([tact_desktop_core::PlanStep {
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
            plan_is_a_single_tool_call(plan_row, plan, &transcript),
            "a one-step plan that is the tool call must not draw a second card"
        );

        let mut multi = (**plan_row).clone();
        multi.body = RowBody::Plan(tact_desktop_core::PlanRow {
            title: Arc::from("Execution plan"),
            steps: Arc::from([
                tact_desktop_core::PlanStep {
                    label: Arc::from("Inspect the store"),
                    status: RowStatus::Success,
                    detail: None,
                },
                tact_desktop_core::PlanStep {
                    label: Arc::from("Run the tests"),
                    status: RowStatus::Running,
                    detail: Some(Arc::from("shell")),
                },
            ]),
        });
        let RowBody::Plan(multi_plan) = &multi.body else {
            unreachable!()
        };
        assert!(
            !plan_is_a_single_tool_call(&multi, multi_plan, &transcript),
            "a multi-step plan keeps its own card"
        );
    }

    #[test]
    fn avatars_use_two_uppercase_initials() {
        assert_eq!(initials_from_name("rg"), "RG");
        assert_eq!(initials_from_name(""), "Y");
        assert_eq!(initials_from_name("rg.agent"), "RG");
    }

    #[test]
    fn run_titles_drop_a_leading_url() {
        assert_eq!(
            strip_leading_url("https://labcoder.github.io/gpui-ai 分析下这个库"),
            "分析下这个库"
        );
        assert_eq!(strip_leading_url("  plain title"), "  plain title");
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
        cx.update(gpui_ai::init);
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
                                .items([PromptAttachment::new("notes.md", "notes.md")]),
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
        cx.update(gpui_ai::init);
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

    #[gpui::test]
    fn sticky_tasks_expand_to_show_every_step(cx: &mut TestAppContext) {
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
            window.debug_bounds("sticky-task-row-1").is_none(),
            "the collapsed strip shows the current task in its header only"
        );
        let toggle = window
            .debug_bounds("toggle-sticky-tasks")
            .expect("sticky task toggle");
        window.simulate_click(toggle.center(), gpui::Modifiers::default());
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("sticky-task-row-1").is_some(),
            "expanding the strip renders every task row"
        );
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
                        .child(sticky_shortcut("Background", InspectorTab::Run, true, cx))
                })
            }
        }
        let data_dir = probe_dir("sticky-background");
        let config = DesktopConfig::load(data_dir.clone()).unwrap();
        install_test_config(&config);
        let (controller, _events) = DesktopController::start(config).unwrap();
        let shutdown = controller.clone();
        let (probe, window) = cx.add_window_view(move |window, cx| {
            gpui_ai::init(cx);
            crate::theme::apply(Appearance::Light, Some(window), cx);
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
        assert!(window.update(|_, cx| probe.read(cx).root.read(cx).overlay.is_some()));
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
}
