use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use gpui::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, ClickEvent, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_ai::prelude::*;
use gpui_component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, StyledExt as _, TitleBar, h_flex, input::Input,
    text::TextView, v_flex,
};
use tact_catalog::{
    ArgumentValue, BuiltinCommand, ChoiceSource, CommandArguments, CommandDescriptor, CommandId,
    SettingId,
};
use tact_desktop_core::{
    Appearance, ArchiveFilter, CommandAvailability, CommandForm, DesktopController, DesktopEffect,
    DesktopEvent, DesktopIntent, DesktopSnapshot, ExtensionsTab, GateCard, GateKind, GateResponse,
    GateSelection, GateState, InspectorTab, PermissionModeView, ProjectId, ReaderRef, RowBody,
    RowId, RowStatus, RunPhase, SectionMask, SessionId, SessionPhase, SettingValue, Submission,
    SurfaceSnapshot, ToolGroupItem, ToolRow, TranscriptRow, TranscriptView, is_enabled,
};

use crate::i18n::{self, TextKey};
use crate::{keymap, theme};

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
    Gate(GateCard),
    Extensions(ExtensionsTab),
    SessionManager,
    Background,
}

struct TranscriptRowView {
    row: Arc<TranscriptRow>,
    snapshot: Arc<DesktopSnapshot>,
    root: WeakEntity<DesktopRoot>,
    assistant_selection: Option<Entity<SelectionActions>>,
}

impl Render for TranscriptRowView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        transcript_row_element(
            &self.row,
            &self.snapshot,
            self.root.clone(),
            self.assistant_selection.clone(),
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
    command_search: Entity<CommandSearch>,
    command_scope: Option<CommandId>,
    form_input: Entity<gpui_component::input::InputState>,
    session_search: Entity<gpui_component::input::InputState>,
    extension_search: Entity<gpui_component::input::InputState>,
    gate_confirm_focus: gpui::FocusHandle,
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
    gate_choices: HashMap<u64, Vec<usize>>,
    gate_keyboard_cursor: usize,
    model_picker_cursor: usize,
    submitted_gates: HashSet<u64>,
    queue_cursor: usize,
    queue_expanded: bool,
    recent_commands: Vec<CommandId>,
    sticky_tasks_collapsed: bool,
    history_cursor: Option<usize>,
    session_project_filter: Option<ProjectId>,
    toast: Option<Toast>,
    focus_restore: crate::ui::shell::FocusRestoration<gpui::FocusHandle>,
    last_surface_revision: Option<String>,
    last_commit_revision: u64,
    last_appearance: Appearance,
    last_language: tact_catalog::CatalogLocale,
    mentions_root: Option<PathBuf>,
    prompt_mentions: Vec<PromptMention>,
    sidebar_query: String,
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
        let scroller = cx.new(|cx| MessageScrollerState::new(snapshot.transcript.len(), cx));
        let prompt = cx.new(|cx| PromptBar::new("tact-composer", window, cx));
        let sidebar_nav = cx.new(|cx| {
            SidebarNav::new("tact-session-nav", window, cx)
                .with_presentation(SidebarNavPresentation::Embedded)
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
        let session_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(i18n::text(snapshot.language, TextKey::SearchSessions))
        });
        let extension_search = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder(i18n::text(snapshot.language, TextKey::SearchExtensions))
        });
        let gate_confirm_focus = cx.focus_handle();
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
            command_search,
            command_scope: None,
            form_input,
            session_search,
            extension_search,
            gate_confirm_focus,
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
            gate_choices: HashMap::new(),
            gate_keyboard_cursor: 0,
            model_picker_cursor: 0,
            submitted_gates: HashSet::new(),
            queue_cursor: 0,
            queue_expanded: false,
            recent_commands: Vec::new(),
            sticky_tasks_collapsed: true,
            history_cursor: None,
            session_project_filter: None,
            toast: None,
            focus_restore: crate::ui::shell::FocusRestoration::default(),
            last_surface_revision: None,
            last_commit_revision: initial_revision,
            last_appearance: Appearance::Light,
            last_language: initial_language,
            mentions_root: None,
            prompt_mentions: Vec::new(),
            sidebar_query: String::new(),
            _subscriptions: vec![
                editor_subscription,
                search_subscription,
                sidebar_subscription,
                extensions_table_subscription,
                session_search_subscription,
                extension_search_subscription,
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
        let _ = self.controller.dispatch(intent);
    }

    fn text(&self, key: TextKey) -> &'static str {
        i18n::text(self.snapshot.language, key)
    }

    fn session_title(&self, session: &tact_desktop_core::SessionSummaryView) -> String {
        if session.message_count == 0 && matches!(session.title.as_str(), "New session" | "新会话")
        {
            self.text(TextKey::NewSession).to_string()
        } else {
            session.title.clone()
        }
    }

    fn sync_sidebar_nav(&self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .snapshot
            .sessions
            .items
            .iter()
            .map(|session| {
                let title = self.session_title(session);
                let locked = session.locked
                    && self.snapshot.session.id.as_ref() != Some(&SessionId(session.id.clone()));
                let badge = if locked {
                    self.text(TextKey::Locked).to_string()
                } else if self.snapshot.language == tact_catalog::CatalogLocale::Zh {
                    format!("{} 条", session.message_count)
                } else {
                    session.message_count.to_string()
                };
                SidebarNavItem::new(session.id.clone(), title)
                    .badge(badge)
                    .disabled(locked)
            })
            .collect::<Vec<_>>();
        let section = SidebarSection::new("sessions", self.text(TextKey::Sessions)).items(items);
        let active = self.snapshot.session.id.as_ref().map(|id| id.0.clone());
        let labels = sidebar_nav_labels(self.snapshot.language);
        self.sidebar_nav.update(cx, |nav, cx| {
            nav.set_labels(labels, window, cx);
            nav.set_sections([section], cx);
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
        self.overlay_focus_pending = true;
    }

    fn close_overlay(&mut self) {
        self.overlay = self.overlay_stack.pop();
    }

    fn clear_overlays(&mut self) {
        self.overlay = None;
        self.overlay_stack.clear();
        self.focus_restore.clear();
        self.focus_restore_pending = false;
    }

    fn handle_event(&mut self, event: DesktopEvent, cx: &mut Context<Self>) {
        match event {
            DesktopEvent::Committed(commit) => {
                let old_rows = self.scroller.read(cx).item_count();
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
                    self.gate_choices.clear();
                    self.submitted_gates.clear();
                    if matches!(self.overlay, Some(Overlay::Gate(_))) {
                        self.clear_overlays();
                    }
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
                self.snapshot = commit.snapshot;
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
                let mut close_resolved_gate = false;
                if let Some(Overlay::Gate(current)) = &mut self.overlay {
                    if let Some(updated) = self
                        .snapshot
                        .pending_gates
                        .iter()
                        .find(|gate| gate.request_id == current.request_id)
                    {
                        *current = updated.clone();
                    } else {
                        if current.status != GateState::Cancelled {
                            current.status = GateState::Resolved;
                        }
                        close_resolved_gate = true;
                    }
                }
                if close_resolved_gate {
                    self.close_overlay();
                    self.focus_restore_pending = true;
                }
                if appearance_changed || self.snapshot.appearance != self.last_appearance {
                    self.last_appearance = self.snapshot.appearance;
                    theme::apply(self.snapshot.appearance, None, cx);
                }
                self.sync_prompt_state(cx);
                self.refresh_typed_reader();
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
                if let Some(Overlay::Reader(reader)) = &mut self.overlay {
                    reader.title = request.title.to_string().into();
                    reader.markdown = request.markdown.to_string().into();
                } else {
                    self.open_overlay(Overlay::Reader(ReaderState {
                        title: request.title.to_string().into(),
                        markdown: request.markdown.to_string().into(),
                        typed: None,
                    }));
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

    #[cfg(feature = "ui-capture")]
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
    pub(crate) fn capture_open_gate(&mut self, gate: tact_desktop_core::GateCard) {
        self.open_overlay(Overlay::Gate(gate));
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
        self.clear_overlays();
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
        self.prompt.update(cx, |prompt, cx| {
            prompt.set_models(models, cx);
            if !current_model.is_empty() {
                prompt.set_selected_model(current_model.to_string(), cx);
            }
            let running = matches!(
                self.snapshot.session.phase,
                SessionPhase::Running | SessionPhase::Starting | SessionPhase::Opening
            );
            prompt.set_progress(
                if running {
                    ProgressState::Running
                } else {
                    ProgressState::Pending
                },
                cx,
            );
            prompt.set_submit(PromptSubmit::Glyph, cx);
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
                        cx.notify();
                    }
                    return true;
                }
                "up" => {
                    let count = self.model_picker_len();
                    if count > 0 {
                        self.model_picker_cursor =
                            self.model_picker_cursor.checked_sub(1).unwrap_or(count - 1);
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

    fn move_gate_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(Overlay::Gate(gate)) = self.overlay.clone() else {
            return;
        };
        let len = gate.options.len();
        if len == 0 {
            return;
        }
        self.gate_keyboard_cursor = if delta < 0 {
            self.gate_keyboard_cursor.checked_sub(1).unwrap_or(len - 1)
        } else {
            (self.gate_keyboard_cursor + 1) % len
        };
        cx.notify();
    }

    fn toggle_gate_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Gate(gate)) = self.overlay.clone() else {
            return;
        };
        let Some(option) = gate
            .options
            .get(self.gate_keyboard_cursor)
            .map(|_| self.gate_keyboard_cursor)
        else {
            return;
        };
        let choices = self.gate_choices.entry(gate.request_id.0).or_default();
        match gate.kind {
            GateKind::Select => {
                *choices = vec![option];
            }
            GateKind::MultiSelect => {
                if let Some(index) = choices.iter().position(|selected| *selected == option) {
                    choices.remove(index);
                } else {
                    choices.push(option);
                    choices.sort_unstable();
                }
            }
        }
        cx.notify();
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
                self.open_overlay(Overlay::Extensions(match command {
                    BuiltinCommand::Mcp => ExtensionsTab::Mcp,
                    BuiltinCommand::Skill => ExtensionsTab::Skills,
                    BuiltinCommand::Plugin => ExtensionsTab::Plugins,
                    BuiltinCommand::Hooks => ExtensionsTab::Hooks,
                    _ => ExtensionsTab::Mcp,
                }));
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
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Hooks));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
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
                self.open_overlay(Overlay::Extensions(ExtensionsTab::Hooks));
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
                self.right_visible = true;
                self.dispatch(DesktopIntent::SetInspector(InspectorTab::Goals));
                self.dispatch(DesktopIntent::InvokeCommand { id, args });
            }
            CommandId::BuiltinPath {
                root: BuiltinCommand::Goal,
                ..
            } => {
                self.right_visible = true;
                self.dispatch(DesktopIntent::SetInspector(InspectorTab::Goals));
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
        self.model_picker_cursor = models
            .iter()
            .position(|model| model.as_ref() == selected)
            .unwrap_or(0);
        self.open_overlay(Overlay::ModelPicker(id));
        cx.notify();
    }

    fn model_picker_len(&self) -> usize {
        match self.overlay {
            Some(Overlay::ModelPicker(SettingId::SubagentModel)) => {
                self.snapshot.composer.subagent_models.len()
            }
            Some(Overlay::ModelPicker(_)) => self.snapshot.composer.models.len(),
            _ => 0,
        }
    }

    fn activate_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::ModelPicker(target)) = self.overlay.clone() else {
            return;
        };
        let models = if target == SettingId::SubagentModel {
            &self.snapshot.composer.subagent_models
        } else {
            &self.snapshot.composer.models
        };
        let Some(value) = models
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
        self.open_overlay(Overlay::Reader(ReaderState {
            title: title.into(),
            markdown: self.text(TextKey::Loading).into(),
            typed: Some(kind),
        }));
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
            reader.markdown = crate::ui::task_dag::dependency_lines(
                &self.snapshot.inspector.tasks,
                self.snapshot.language,
            )
            .join("\n")
            .into();
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
                        args: CommandArguments {
                            tokens: vec!["create".into(), text.trim().into()],
                        },
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
                    args: CommandArguments {
                        tokens: vec!["create".into(), value],
                    },
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
        let items = entries
            .into_iter()
            .map(|entry| {
                let mut keywords = entry.aliases.clone();
                keywords.push(entry.path.join(" "));
                CommandSearchItem::new(
                    command_key(&entry.id),
                    entry.label.get(self.snapshot.language),
                )
                .subtitle(entry.description.get(self.snapshot.language).to_string())
                .keywords(keywords)
                .disabled(!is_enabled(entry, availability))
            })
            .collect::<Vec<_>>();
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
        if self.overlay.is_some() {
            self.close_overlay();
            self.restore_focus(window, cx);
            cx.notify();
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
        let project = self.snapshot.project.display_name.clone();
        let phase =
            crate::ui::title_bar::status_text(&self.snapshot.session, self.snapshot.language);
        let tone = match self.snapshot.session.phase {
            SessionPhase::Ready | SessionPhase::Running => StatusTone::Success,
            SessionPhase::Opening | SessionPhase::Starting | SessionPhase::Switching => {
                StatusTone::Info
            }
            SessionPhase::Disconnected { .. } | SessionPhase::Failed { .. } => StatusTone::Danger,
            SessionPhase::Empty | SessionPhase::Closing => StatusTone::Neutral,
        };
        let active = matches!(
            self.snapshot.session.phase,
            SessionPhase::Opening | SessionPhase::Starting | SessionPhase::Switching
        );
        let session_label = self
            .snapshot
            .session
            .id
            .as_ref()
            .map(|id| {
                let raw = id.0.to_string();
                format!("session {}", &raw[..raw.len().min(4)])
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
                    .gap_2()
                    .items_center()
                    .px_3()
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
                                    .size(px(24.))
                                    .rounded_md()
                                    .bg(cx.theme().primary)
                                    .text_color(cx.theme().primary_foreground)
                                    .text_sm()
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
                                            .text_sm()
                                            .font_semibold()
                                            .line_height(px(16.))
                                            .child("tact"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .line_height(px(12.))
                                            .child(self.text(TextKey::DesktopPreview)),
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
                            .gap_2()
                            .items_center()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().popover)
                            .px_3()
                            .py_1()
                            .hover(|style| style.bg(cx.theme().muted))
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
                        StatusBadge::new("tact-title-status", phase)
                            .tone(tone)
                            .active(active),
                    )
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
                                Icon::new(IconName::Search)
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
        let project = self.snapshot.project.display_name.clone();
        self.sync_sidebar_nav(window, cx);

        v_flex()
            .id("session-rail")
            .role(Role::Navigation)
            .aria_label(self.text(TextKey::ProjectsAndSessions))
            .w(px(f32::from(geometry.rail_width) * self.snapshot.text_scale))
            .h_full()
            .flex_none()
            .p_2()
            .gap_2()
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
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().sidebar_border)
                    .bg(cx.theme().popover)
                    .p_2()
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        div()
                            .flex_none()
                            .size(px(28.))
                            .rounded_md()
                            .bg(cx.theme().secondary)
                            .text_xs()
                            .font_semibold()
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
                                    .text_sm()
                                    .font_semibold()
                                    .child(project.to_string()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} · {}",
                                        self.text(TextKey::Sessions),
                                        self.snapshot.sessions.items.len()
                                    )),
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
                    .gap_1()
                    .items_center()
                    .child(filter_button(
                        self.text(TextKey::Active),
                        self.snapshot.sessions.filter == ArchiveFilter::Active,
                        ArchiveFilter::Active,
                        cx,
                    ))
                    .child(filter_button(
                        self.text(TextKey::Archived),
                        self.snapshot.sessions.filter == ArchiveFilter::Archived,
                        ArchiveFilter::Archived,
                        cx,
                    ))
                    .child(filter_button(
                        self.text(TextKey::All),
                        self.snapshot.sessions.filter == ArchiveFilter::All,
                        ArchiveFilter::All,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("resume-last")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Resume))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(22.))
                            .rounded_md()
                            .text_color(cx.theme().muted_foreground)
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                Icon::new(IconName::RotateCw)
                                    .size(px(13.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                this.dispatch(DesktopIntent::ResumeLast);
                            })),
                    ),
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
                    .text_xs()
                    .text_color(cx.theme().link)
                    .child(self.text(TextKey::ManageSessions))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.remember_focus(window, cx);
                        this.session_project_filter = this.snapshot.project.id.clone();
                        this.session_search.update(cx, |input, cx| {
                            input.focus(window, cx);
                        });
                        this.open_overlay(Overlay::SessionManager);
                        this.overlay_focus_pending = false;
                    })),
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
                    .min_h_0()
                    .flex_1()
                    .child(self.sidebar_nav.clone()),
            )
            .child(div().flex_1())
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .px_1()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::Extensions)),
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
            .when_some(self.account_summary(), |rail, (label, detail, ratio)| {
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
                                .child(div().flex_1().text_xs().font_semibold().child(label))
                                .child(
                                    div()
                                        .text_xs()
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
                        ),
                )
            })
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

    fn account_summary(&self) -> Option<(String, String, f32)> {
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
                let detail = match (used, limit) {
                    (Some(used), Some(limit)) => format!("{used}/{limit}"),
                    _ => resets_at.as_deref().unwrap_or("").to_string(),
                };
                Some((label, detail, ratio))
            }
            tact_desktop_core::AccountView::Balance { total, .. } => Some((
                self.text(TextKey::Balance).to_string(),
                total.to_string(),
                1.0,
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
            let key = (scope.to_string(), row.id);
            if let Some(selection) = self.assistant_selections.get(&key) {
                selection.update(cx, |selection, cx| {
                    selection.set_markdown(assistant.markdown.to_string(), cx);
                });
                continue;
            }
            let selection = cx.new(|cx| {
                SelectionActions::new(
                    format!("assistant-selection-{}", row.ordinal),
                    assistant.markdown.to_string(),
                    window,
                    cx,
                )
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
        let title = transcript
            .iter()
            .find_map(|row| match &row.body {
                RowBody::User(user) => Some(user.text.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| self.snapshot.session.title.to_string());
        let session_label = self
            .snapshot
            .session
            .id
            .as_ref()
            .map(|id| {
                let raw = id.0.to_string();
                raw[..raw.len().min(4)].to_uppercase()
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
        .p_5();
        v_flex()
            .id("transcript-column")
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                v_flex()
                    .flex_none()
                    .gap_2()
                    .px_8()
                    .pt_6()
                    .pb_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
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
                            .child(
                                div().font_family("Lilex").child(format!(
                                    "{elapsed} {}",
                                    self.text(TextKey::ElapsedLabel)
                                )),
                            )
                            .child(
                                div()
                                    .font_family("Lilex")
                                    .child(format!("{} {turns}", self.text(TextKey::TurnLabel))),
                            ),
                    )
                    .child({
                        // Long CJK prompts use the compact display scale and a
                        // 1.12 line height instead of the Latin hero title.
                        let cjk_title = is_cjk_title(self.snapshot.language, &title);
                        div()
                            .max_w(px(900.))
                            .font_semibold()
                            .when(cjk_title, |title| title.text_xl().line_height(px(22.4)))
                            .when(!cjk_title, |title| title.text_2xl().line_height(px(34.)))
                            .child(title)
                    }),
            )
            .child(div().flex_1().min_h_0().child(scroller))
            .into_any_element()
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
            let needs_update = self.pending_row_updates.contains(&row.id)
                || !self.transcript_rows.contains_key(&row.id);
            let view = if let Some(view) = self.transcript_rows.get(&row.id).cloned() {
                let context_changed = view.read(cx).snapshot.appearance != snapshot.appearance
                    || view.read(cx).snapshot.language != snapshot.language
                    || view.read(cx).snapshot.show_hook_output != snapshot.show_hook_output;
                if needs_update || context_changed {
                    view.update(cx, |view, cx| {
                        view.row = row.clone();
                        view.snapshot = snapshot.clone();
                        view.assistant_selection = selection;
                        cx.notify();
                    });
                }
                view
            } else {
                let root = cx.weak_entity();
                let view = cx.new(|_| TranscriptRowView {
                    row: row.clone(),
                    snapshot: snapshot.clone(),
                    root,
                    assistant_selection: selection,
                });
                self.transcript_rows.insert(row.id, view.clone());
                view
            };
            views.push(view);
        }
        self.pending_row_updates.clear();
        views
    }

    fn render_inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = crate::ui::shell::geometry(self.snapshot.appearance);
        let tabs = [
            (InspectorTab::Run, self.text(TextKey::Run)),
            (InspectorTab::Goals, self.text(TextKey::Goals)),
            (InspectorTab::Subagents, self.text(TextKey::Subagents)),
            (InspectorTab::Context, self.text(TextKey::Context)),
        ];
        let tab_buttons = tabs
            .into_iter()
            .enumerate()
            .map(|(index, (tab, label))| {
                let selected = self.snapshot.inspector.tab == tab;
                div()
                    .id(SharedString::from(format!("inspector-tab-{index}")))
                    .role(Role::Tab)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                    .aria_selected(selected)
                    .aria_label(label)
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(selected, |tab| tab.bg(cx.theme().accent))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                        this.dispatch(DesktopIntent::SetInspector(tab));
                    }))
            })
            .collect::<Vec<_>>();
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
            .role(Role::Complementary)
            .aria_label(self.text(TextKey::RunInspector))
            .w(px(
                f32::from(geometry.inspector_width) * self.snapshot.text_scale
            ))
            .h_full()
            .flex_none()
            .p_3()
            .gap_3()
            .font_family(theme::native_font_family())
            .border_l(px(theme::border_width()))
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .id("inspector-tabs")
                    .role(Role::TabList)
                    .aria_label(self.text(TextKey::RunInspector))
                    .gap_1()
                    .children(tab_buttons),
            )
            .child(content)
    }

    fn render_run_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let runtime = &self.snapshot.runtime;
        let tasks = self
            .snapshot
            .inspector
            .tasks
            .iter()
            .map(|task| {
                let status = match task.status {
                    tact_desktop_core::TaskStatusSnapshot::Pending => TodoStatus::Pending,
                    tact_desktop_core::TaskStatusSnapshot::InProgress => TodoStatus::Active,
                    tact_desktop_core::TaskStatusSnapshot::Completed => TodoStatus::Done,
                };
                TodoItem::new(
                    SharedString::from(format!("inspector-task-{}", task.id)),
                    task.subject.to_string(),
                )
                .status(status)
            })
            .collect::<Vec<_>>();
        let background = self.background_task_rows(cx);
        v_flex()
            .id("run-inspector-content")
            .role(Role::Region)
            .aria_label(self.text(TextKey::Run))
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(section_label(self.text(TextKey::Run), cx))
                    .child(metric(
                        self.text(TextKey::Phase),
                        run_phase_label(runtime.current_phase, self.snapshot.language),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::Session),
                        crate::ui::title_bar::status_text(
                            &self.snapshot.session,
                            self.snapshot.language,
                        ),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::Turns),
                        &runtime.turns_taken.to_string(),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::LatestTool),
                        runtime.latest_tool.as_deref().unwrap_or("—"),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::Tokens),
                        &runtime.usage.total_tokens.to_string(),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::Model),
                        runtime.model.as_deref().unwrap_or("—"),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::Elapsed),
                        &runtime
                            .run_started_at
                            .map(|started| {
                                chrono::Utc::now()
                                    .signed_duration_since(started)
                                    .num_milliseconds()
                                    .max(0)
                                    .to_string()
                            })
                            .unwrap_or_else(|| "0".into()),
                        cx,
                    ))
                    .child(metric(
                        self.text(TextKey::CacheRate),
                        &cache_rate(&runtime.usage),
                        cx,
                    ))
                    .when_some(runtime.latest_tool.as_ref(), |column, tool| {
                        column.child(
                            ToolChip::new("inspector-latest-tool", tool.to_string()).status(
                                match runtime.current_phase {
                                    RunPhase::Idle | RunPhase::Complete => ToolStatus::Success,
                                    RunPhase::Failed => ToolStatus::Failed,
                                    RunPhase::Waiting
                                    | RunPhase::Thinking
                                    | RunPhase::Streaming
                                    | RunPhase::RunningTools
                                    | RunPhase::WaitingForGate
                                    | RunPhase::Cancelling => ToolStatus::Running,
                                },
                            ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section_label(self.text(TextKey::Tasks), cx))
                    .child(
                        TodoList::new("inspector-todos")
                            .title(self.text(TextKey::Tasks))
                            .completion_label(match self.snapshot.language {
                                tact_catalog::CatalogLocale::En => "complete",
                                tact_catalog::CatalogLocale::Zh => "已完成",
                            })
                            .items(tasks),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section_label(self.text(TextKey::Background), cx))
                    .children(background),
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
                    .child(div().truncate().text_sm().child(task.command.to_string()))
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

    fn render_context_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let usage = &self.snapshot.inspector.context;
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
            .into_any_element()
    }

    fn render_sticky(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = crate::ui::sticky::pending_gate(&self.snapshot.pending_gates).cloned();
        // The counter always describes the whole plan; collapsing only hides
        // rows, it does not make the plan smaller.
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
        let tasks = if self.sticky_tasks_collapsed {
            crate::ui::sticky::active_task(&self.snapshot.inspector)
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            all_tasks.iter().collect::<Vec<_>>()
        };
        let task_rows = tasks
            .into_iter()
            .map(|task| {
                let snapshot =
                    TaskSnapshot::new(format!("sticky-task-{}", task.id), task.subject.to_string())
                        .detail(task_status_label(task.status, self.snapshot.language));
                let progressive = match task.status {
                    tact_desktop_core::TaskStatusSnapshot::Pending => {
                        Progressive::pending(snapshot)
                    }
                    tact_desktop_core::TaskStatusSnapshot::InProgress => {
                        Progressive::running(snapshot)
                    }
                    tact_desktop_core::TaskStatusSnapshot::Completed => {
                        Progressive::complete(snapshot)
                    }
                };
                TaskRow::new(&progressive)
                    .max_w(px(220.))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        h_flex()
            .id("sticky-strip")
            .debug_selector(|| "sticky-strip".into())
            .role(Role::Status)
            .aria_label(self.text(TextKey::RunStatus))
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .when(task_total > 0, |strip| {
                strip.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} {completed}/{task_total}",
                                    self.text(TextKey::Tasks).to_uppercase()
                                )),
                        )
                        .child(
                            div()
                                .h(px(3.))
                                .w(px(72.))
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(px(72.0 * completed as f32 / task_total as f32))
                                        .rounded_full()
                                        .bg(cx.theme().success),
                                ),
                        ),
                )
            })
            .when_some(pending, |strip, gate| {
                let gate_for_open = gate.clone();
                strip.child(
                    h_flex()
                        .flex_1()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().warning)
                        .bg(cx.theme().warning.opacity(0.08))
                        .px_2()
                        .py_1()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_sm()
                                .child(gate.prompt.to_string()),
                        )
                        .child(
                            div()
                                .id("review-gate")
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
                                                    tact_desktop_core::GateOrigin::Tool {
                                                        tool_id,
                                                    }
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
                                    if let Some(index) =
                                        this.snapshot.transcript.iter().position(|row| {
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
                                        })
                                    {
                                        this.scroller.update(cx, |state, cx| {
                                            state.scroll_to_item(index, cx);
                                        });
                                    }
                                    let mut selected = match &gate.selection {
                                        GateSelection::Single(index) => {
                                            index.iter().copied().collect()
                                        }
                                        GateSelection::Multiple(indexes) => indexes.clone(),
                                    };
                                    if selected.is_empty() && !gate.options.is_empty() {
                                        selected.push(0);
                                    }
                                    this.gate_choices.insert(gate.request_id.0, selected);
                                    this.gate_keyboard_cursor = 0;
                                    if let Some((group_id, collapsed, tool_to_expand)) = group {
                                        if let Some(tool_id) = tool_to_expand {
                                            this.dispatch(DesktopIntent::ToggleTool {
                                                group: group_id,
                                                tool: tool_id,
                                            });
                                        }
                                        if collapsed {
                                            this.dispatch(DesktopIntent::ToggleGroup {
                                                id: group_id,
                                            });
                                        }
                                    }
                                    this.open_overlay(Overlay::Gate(gate_for_open.clone()));
                                    this.gate_confirm_focus.focus(window, cx);
                                    this.overlay_focus_pending = false;
                                })),
                        ),
                )
            })
            .child(
                h_flex()
                    .id("sticky-task-list")
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .overflow_x_scroll()
                    .children(task_rows),
            )
            .when(self.snapshot.inspector.tasks.len() > 1, |strip| {
                strip.child(
                    div()
                        .flex_none()
                        .id("toggle-sticky-tasks")
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
                        .text_xs()
                        .child(if self.sticky_tasks_collapsed {
                            self.text(TextKey::Expand)
                        } else {
                            self.text(TextKey::Collapse)
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.sticky_tasks_collapsed = !this.sticky_tasks_collapsed;
                            cx.notify();
                        })),
                )
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(sticky_shortcut(
                        self.text(TextKey::Goals),
                        InspectorTab::Goals,
                        false,
                        cx,
                    ))
                    .child(sticky_shortcut(
                        self.text(TextKey::Subagents),
                        InspectorTab::Subagents,
                        false,
                        cx,
                    ))
                    .child(sticky_shortcut(
                        self.text(TextKey::Background),
                        InspectorTab::Run,
                        true,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_back_to_bottom(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = format!(
            "{} · {}",
            self.text(TextKey::BackToBottom),
            self.text(TextKey::NewActivity)
        );
        div()
            .id("back-to-bottom")
            .debug_selector(|| "back-to-bottom".into())
            .absolute()
            .right_4()
            .bottom_4()
            .occlude()
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
            .text_color(cx.theme().link)
            .shadow_md()
            .child(
                h_flex()
                    .gap_1()
                    .child(self.text(TextKey::BackToBottom))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("· {}", self.text(TextKey::NewActivity))),
                    ),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_end(cx));
                this.dispatch(DesktopIntent::BackToBottom);
            }))
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
        h_flex()
            .id("permission-picker")
            .debug_selector(|| "permission-picker".into())
            .role(Role::Group)
            .aria_label(self.text(TextKey::PermissionMode))
            .gap_1()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.text(TextKey::PermissionMode)),
            )
            .child(div().flex_1())
            .children(choices.into_iter().map(|(mode, value, label)| {
                gpui_base::Button::new(SharedString::from(format!("permission-{value}")))
                    .role(Role::RadioButton)
                    .tab_stop(true)
                    .aria_label(label)
                    .selected(mode == selected)
                    .cursor_pointer()
                    .rounded_md()
                    .border_1()
                    .border_color(if mode == selected {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .when(mode == selected, |button| button.bg(cx.theme().accent))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                        this.dispatch(DesktopIntent::SetSetting {
                            id: SettingId::PermissionMode,
                            value: SettingValue::Choice(value.into()),
                        });
                    }))
            }))
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
            .px_6()
            .pt_2()
            .pb_4()
            .border_t_1()
            .border_color(cx.theme().border)
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
                            .child(queue_view),
                    )
                })
            })
            .child(self.render_permission_picker(cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(composer_insert_button(
                        "composer-mention",
                        "@",
                        self.text(TextKey::MentionFile),
                        "@",
                        cx,
                    ))
                    .child(composer_insert_button(
                        "composer-slash",
                        "/",
                        self.text(TextKey::InsertCommand),
                        "/",
                        cx,
                    )),
            )
            .when_some(self.slash_overlay.clone(), |area, overlay| {
                area.key_context("SlashOverlay")
                    .child(self.render_slash_overlay(&overlay, cx))
            })
            .child(self.prompt.clone())
            .into_any_element()
    }

    fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.overlay.clone()?;
        match overlay {
            Overlay::CommandCenter => {
                let compact = f32::from(window.viewport_size().height) < 420.0;
                Some(
                    v_flex()
                        .id("command-center-overlay")
                        .role(Role::Dialog)
                        .aria_label(self.text(TextKey::CommandCenter))
                        .w(px(
                            (f32::from(window.viewport_size().width) - 24.0).clamp(320.0, 640.0)
                        ))
                        .max_h(overlay_height(window, 560.))
                        .gap_3()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .shadow_lg()
                        .overflow_y_scroll()
                        .when(!compact, |dialog| {
                            dialog.child(
                                h_flex()
                                    .flex_none()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .font_semibold()
                                            .child(self.text(TextKey::CommandCenter)),
                                    )
                                    .child(
                                        div()
                                            .rounded_md()
                                            .bg(cx.theme().muted)
                                            .px_1p5()
                                            .py_0p5()
                                            .text_xs()
                                            .font_family("Lilex")
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Esc"),
                                    ),
                            )
                        })
                        .child(self.command_search.clone())
                        .when(!compact, |dialog| {
                            dialog.child(
                                h_flex()
                                    .flex_none()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("↑↓")
                                    .child("↵")
                                    .child("Esc"),
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
            Overlay::Reader(reader) => Some(
                v_flex()
                    .id("reader-overlay")
                    .role(Role::Dialog)
                    .aria_label(reader.title.clone())
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
                            .child(div().font_semibold().child(reader.title.clone()))
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
                    .child(match reader.typed {
                        Some(ReaderKind::Balance) => {
                            InsightCard::new("balance-reader-card", reader.title.to_string())
                                .body(reader.markdown.to_string())
                                .into_any_element()
                        }
                        _ => TextView::markdown("reader-markdown", reader.markdown.to_string())
                            .into_any_element(),
                    })
                    .into_any_element(),
            ),
            Overlay::ModelPicker(id) => Some(self.render_model_picker(id, window, cx)),
            Overlay::Subagent(child_id) => Some(self.render_subagent_overlay(child_id, window, cx)),
            Overlay::Gate(gate) => Some(self.render_gate_overlay(gate, window, cx)),
            Overlay::Extensions(tab) => Some(self.render_extensions_overlay(tab, window, cx)),
            Overlay::SessionManager => Some(self.render_session_manager_overlay(window, cx)),
            Overlay::Background => Some(self.render_background_overlay(window, cx)),
        }
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
        let (models, selected) = if target == SettingId::SubagentModel {
            (
                self.snapshot.composer.subagent_models.clone(),
                self.snapshot.composer.subagent_model.clone(),
            )
        } else {
            (
                self.snapshot.composer.models.clone(),
                self.snapshot.composer.model.clone(),
            )
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
                .rounded_md()
                .border_1()
                .border_color(if active {
                    cx.theme().primary
                } else {
                    cx.theme().border
                })
                .px_2()
                .py_1()
                .when(chosen, |row| row.bg(cx.theme().accent))
                .child(value.clone())
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.model_picker_cursor = index;
                    this.activate_model_picker(window, cx);
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();
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
            .when(rows.is_empty(), |dialog| {
                dialog.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.text(TextKey::NoModels)),
                )
            })
            .child(v_flex().gap_1().overflow_y_scrollbar().children(rows))
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
        let selections = transcript
            .as_ref()
            .map(|transcript| self.sync_assistant_selections(transcript, &child_id, window, cx))
            .unwrap_or_default();
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
                            selections.get(&(child_id.clone(), row.id)).cloned(),
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
        v_flex()
            .id("subagent-overlay")
            .role(Role::Dialog)
            .aria_label(format!(
                "{} {child_id}",
                self.text(TextKey::SubagentTranscript)
            ))
            .w(overlay_width(window, 820.))
            .max_h(px(720.))
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
                    .child(div().font_semibold().child(format!(
                        "{} {child_id}",
                        self.text(TextKey::SubagentTranscript)
                    )))
                    .child(
                        div()
                            .id("close-subagent")
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                            .aria_label(self.text(TextKey::Close))
                            .cursor_pointer()
                            .child(self.text(TextKey::Close))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_overlay();
                                this.restore_focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("subagent-transcript")
                    .gap_2()
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

    fn render_gate_overlay(
        &mut self,
        gate: GateCard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self
            .gate_choices
            .entry(gate.request_id.0)
            .or_insert_with(|| match &gate.selection {
                GateSelection::Single(index) => index.iter().copied().collect(),
                GateSelection::Multiple(indexes) => indexes.clone(),
            })
            .clone();
        let pending = gate.status == GateState::Pending;
        let request_id = gate.request_id.0;
        let options = gate
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let is_selected = selected.contains(&index);
                let is_active = index == self.gate_keyboard_cursor;
                gpui_base::Button::new(("gate-option", u64::try_from(index).unwrap_or(u64::MAX)))
                    .role(match gate.kind {
                        GateKind::Select => Role::RadioButton,
                        GateKind::MultiSelect => Role::CheckBox,
                    })
                    .aria_label(option.to_string())
                    .selected(is_selected)
                    .rounded_md()
                    .border_1()
                    .border_color(if is_active {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .px_3()
                    .py_2()
                    .when(is_selected, |row| row.bg(cx.theme().accent))
                    .child(option.to_string())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.gate_keyboard_cursor = index;
                        let choices = this.gate_choices.entry(request_id).or_default();
                        match gate.kind {
                            GateKind::Select => {
                                choices.clear();
                                choices.push(index);
                            }
                            GateKind::MultiSelect => {
                                if let Some(position) =
                                    choices.iter().position(|value| *value == index)
                                {
                                    choices.remove(position);
                                } else {
                                    choices.push(index);
                                }
                            }
                        }
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        let origin = match &gate.origin {
            tact_desktop_core::GateOrigin::Main => {
                i18n::text(self.snapshot.language, TextKey::MainAgent).to_string()
            }
            tact_desktop_core::GateOrigin::Subagent { child_id, tool_id } => match tool_id {
                Some(tool_id) => format!(
                    "{} {child_id} · {} {tool_id}",
                    self.text(TextKey::SubagentTranscript),
                    self.text(TextKey::Tool)
                ),
                None => format!("{} {child_id}", self.text(TextKey::SubagentTranscript)),
            },
            tact_desktop_core::GateOrigin::Tool { tool_id } => {
                format!("{} {tool_id}", self.text(TextKey::Tool))
            }
        };
        let gate_for_confirm = gate.clone();
        let selected_for_confirm = selected.clone();
        v_flex()
            .id("gate-overlay")
            .key_context("Gate")
            .role(Role::Dialog)
            .aria_label(gate.prompt.to_string())
            .w(overlay_width(window, 560.))
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().warning)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(origin),
                    )
                    .child(div().font_semibold().child(gate.prompt.to_string())),
            )
            .child(v_flex().gap_2().children(options))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .when(pending, |row| {
                        row.child(
                            gpui_base::Button::new("cancel-gate")
                                .aria_label(self.text(TextKey::CancelRequest))
                                .px_2()
                                .py_1()
                                .child(self.text(TextKey::Cancel))
                                .on_click(cx.listener({
                                    let gate = gate_for_confirm.clone();
                                    move |this, _: &ClickEvent, window, cx| {
                                        this.dispatch(DesktopIntent::RespondGate {
                                            session_epoch: gate.session_epoch,
                                            request_id: gate.request_id.0,
                                            response: match gate.kind {
                                                GateKind::Select => GateResponse::Select(None),
                                                GateKind::MultiSelect => {
                                                    GateResponse::MultiSelect(None)
                                                }
                                            },
                                        });
                                        this.submitted_gates.insert(gate.request_id.0);
                                        this.close_overlay();
                                        this.restore_focus(window, cx);
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .when(pending, |row| {
                        row.child(
                            gpui_base::Button::new("confirm-gate")
                                .aria_label(self.text(TextKey::ConfirmRequest))
                                .track_focus(&self.gate_confirm_focus)
                                .disabled(selected_for_confirm.is_empty())
                                .rounded_md()
                                .bg(cx.theme().primary)
                                .text_color(cx.theme().primary_foreground)
                                .px_3()
                                .py_1()
                                .child(self.text(TextKey::Confirm))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    if selected_for_confirm.is_empty() {
                                        return;
                                    }
                                    this.dispatch(DesktopIntent::RespondGate {
                                        session_epoch: gate_for_confirm.session_epoch,
                                        request_id: gate_for_confirm.request_id.0,
                                        response: match gate_for_confirm.kind {
                                            GateKind::Select => GateResponse::Select(
                                                selected_for_confirm.first().copied(),
                                            ),
                                            GateKind::MultiSelect => GateResponse::MultiSelect(
                                                Some(selected_for_confirm.clone()),
                                            ),
                                        },
                                    });
                                    this.submitted_gates.insert(gate_for_confirm.request_id.0);
                                    this.close_overlay();
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_extensions_overlay(
        &mut self,
        tab: ExtensionsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_table_rows = if self.extension_manage {
            self.sync_extensions_table(tab, window, cx)
        } else {
            true
        };
        let query = self.extension_query.clone();
        let matches = |value: &str| crate::ui::extensions::matches_extension(&query, value);
        let tabs = [
            (ExtensionsTab::Mcp, self.text(TextKey::Mcp)),
            (ExtensionsTab::Skills, self.text(TextKey::Skills)),
            (ExtensionsTab::Plugins, self.text(TextKey::Plugins)),
            (ExtensionsTab::Hooks, self.text(TextKey::Hooks)),
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
                .rounded_md()
                .px_2()
                .py_1()
                .text_xs()
                .when(candidate == tab, |button| button.bg(cx.theme().accent))
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

        v_flex()
            .id("extensions-overlay")
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::Extensions))
            .w(overlay_width(window, 720.))
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
                    .items_center()
                    .child(div().font_semibold().child(if self.extension_manage {
                        self.text(TextKey::ManageExtensions)
                    } else {
                        self.text(TextKey::Extensions)
                    }))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .id("manage-extensions")
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(if self.extension_manage {
                                        self.text(TextKey::BackToExtensions)
                                    } else {
                                        self.text(TextKey::ManageExtensions)
                                    })
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(cx.theme().link)
                                    .child(if self.extension_manage {
                                        self.text(TextKey::Back)
                                    } else {
                                        self.text(TextKey::Manage)
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if this.extension_manage {
                                            this.extension_manage = false;
                                        } else {
                                            let mut view = this.snapshot.extensions.clone();
                                            view.query = this.extension_query.clone();
                                            let dialog = crate::ui::extensions::manage(&view);
                                            this.extension_query = dialog.query;
                                            this.extension_manage = true;
                                        }
                                        cx.notify();
                                    })),
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
                                    .child(self.text(TextKey::Close))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.close_overlay();
                                        this.restore_focus(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(h_flex().gap_1().children(tabs))
            .child(Input::new(&self.extension_search))
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
                        .child(self.text(TextKey::NoItems))
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
        let manager_view = crate::ui::session_manager::SessionManagerView {
            project_filter: self
                .session_project_filter
                .as_ref()
                .map(|project| project.0.clone()),
            filter: self.snapshot.sessions.filter,
            query: self.snapshot.sessions.query.clone(),
        };
        let rows = crate::ui::session_manager::filter_sessions(
            &self.snapshot.sessions.items,
            &manager_view,
        )
        .into_iter()
        .map(|session| {
            let session_id = SessionId(session.id.clone());
            let rename_id = session_id.clone();
            let archive_id = session_id.clone();
            let archived = session.archived_at.is_some();
            let rename_value = self.session_title(session);
            let title = rename_value.clone();
            let locked = session.locked && self.snapshot.session.id.as_ref() != Some(&session_id);
            let title = if locked {
                format!("{title} · {}", self.text(TextKey::Locked))
            } else {
                title
            };
            let resume_id = session_id.clone();
            h_flex()
                .id(SharedString::from(format!(
                    "manager-session-{}",
                    session.id
                )))
                .justify_between()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .p_2()
                .child(
                    v_flex()
                        .id(SharedString::from(format!("resume-session-{}", session.id)))
                        .role(Role::Button)
                        .tab_stop(!locked)
                        .aria_label(title.clone())
                        .when(!locked, |row| {
                            row.cursor_pointer().on_click(cx.listener(
                                move |this, _: &ClickEvent, window, cx| {
                                    this.dispatch(DesktopIntent::ResumeSession {
                                        session_id: resume_id.clone(),
                                    });
                                    this.close_overlay();
                                    this.restore_focus(window, cx);
                                    cx.notify();
                                },
                            ))
                        })
                        .gap_0p5()
                        .child(div().text_sm().child(title))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} · {}",
                                    session.updated_at.format("%Y-%m-%d %H:%M"),
                                    match (self.snapshot.language, session.message_count) {
                                        (tact_catalog::CatalogLocale::Zh, count) => {
                                            format!("{count} 条消息")
                                        }
                                        (_, 1) => "1 message".to_string(),
                                        (_, count) => format!("{count} messages"),
                                    }
                                )),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .id(SharedString::from(format!("rename-session-{}", session.id)))
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(rename_label)
                                .cursor_pointer()
                                .text_xs()
                                .child(self.text(TextKey::Rename))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
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
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "archive-session-{}",
                                    session.id
                                )))
                                .role(Role::Button)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                .aria_label(if archived {
                                    restore_label
                                } else {
                                    archive_label
                                })
                                .cursor_pointer()
                                .text_xs()
                                .child(if archived {
                                    restore_label
                                } else {
                                    archive_label
                                })
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                    this.dispatch(DesktopIntent::SetSessionArchived {
                                        session_id: archive_id.clone(),
                                        archived: !archived,
                                    });
                                })),
                        ),
                )
        })
        .collect::<Vec<_>>();
        let project_rows = self
            .snapshot
            .projects
            .iter()
            .map(|project| {
                let selected = project.id == self.session_project_filter;
                let root = project.root.clone();
                let project_id = project.id.clone();
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
                .rounded_md()
                .px_2()
                .py_1()
                .text_xs()
                .child(project.display_name.to_string())
                .on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.session_project_filter = project_id.clone();
                        if this.snapshot.project.id != project_id
                            && let Some(root) = root.clone()
                        {
                            this.dispatch(DesktopIntent::OpenProject { root });
                            this.close_overlay();
                            this.restore_focus(window, cx);
                        }
                    },
                ))
            })
            .collect::<Vec<_>>();

        v_flex()
            .id("session-manager-overlay")
            .role(Role::Dialog)
            .aria_label(self.text(TextKey::SessionManager))
            .w(overlay_width(window, 760.))
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
                    .child(
                        div()
                            .font_semibold()
                            .child(self.text(TextKey::SessionManager)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("manager-new-session")
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                                    .aria_label(self.text(TextKey::NewSession))
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(cx.theme().link)
                                    .child(self.text(TextKey::NewSession))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.dispatch(DesktopIntent::NewSession);
                                        this.close_overlay();
                                        this.restore_focus(window, cx);
                                        cx.notify();
                                    })),
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
                                    .child(self.text(TextKey::Close))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.close_overlay();
                                        this.restore_focus(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.text(TextKey::Projects)),
                    )
                    .child(
                        h_flex()
                            .id("session-manager-projects")
                            .gap_1()
                            .flex_wrap()
                            .children(project_rows),
                    ),
            )
            .child(Input::new(&self.session_search))
            .child(
                h_flex()
                    .gap_1()
                    .child(filter_button(
                        self.text(TextKey::Active),
                        self.snapshot.sessions.filter == ArchiveFilter::Active,
                        ArchiveFilter::Active,
                        cx,
                    ))
                    .child(filter_button(
                        self.text(TextKey::Archived),
                        self.snapshot.sessions.filter == ArchiveFilter::Archived,
                        ArchiveFilter::Archived,
                        cx,
                    ))
                    .child(filter_button(
                        self.text(TextKey::All),
                        self.snapshot.sessions.filter == ArchiveFilter::All,
                        ArchiveFilter::All,
                        cx,
                    )),
            )
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
            .when(self.snapshot.sessions.loading, |manager| {
                manager.child(LoadingState::new().label(match self.snapshot.language {
                    tact_catalog::CatalogLocale::En => "Loading sessions",
                    tact_catalog::CatalogLocale::Zh => "正在加载会话",
                }))
            })
            .child(
                v_flex()
                    .id("session-manager-list")
                    .gap_1()
                    .overflow_y_scroll()
                    .children(rows),
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
            self.session_search.update(cx, |input, cx| {
                input.set_placeholder(sessions_placeholder, window, cx);
            });
            self.extension_search.update(cx, |input, cx| {
                input.set_placeholder(extensions_placeholder, window, cx);
            });
        }
        if self.overlay_focus_pending && self.overlay.is_some() {
            self.overlay_focus_pending = false;
            if matches!(self.overlay, Some(Overlay::Extensions(_))) {
                self.extension_search
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
        let overlay = self.render_overlay(window, cx);
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
            .on_action(cx.listener(|this, _: &keymap::ToggleLeft, _, cx| {
                this.left_visible = !this.left_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleRight, _, cx| {
                this.right_visible = !this.right_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::BackToBottom, _, cx| {
                this.scroller
                    .update(cx, |state, cx| state.scroll_to_end(cx));
                this.dispatch(DesktopIntent::BackToBottom);
            }))
            .on_action(cx.listener(|this, _: &keymap::CancelRun, window, cx| {
                this.request_cancel_run(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::NextInspector, _, cx| {
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(next_inspector(
                    this.snapshot.inspector.tab,
                )));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::PreviousInspector, _, cx| {
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(previous_inspector(
                    this.snapshot.inspector.tab,
                )));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowRun, _, cx| {
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(InspectorTab::Run));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowGoals, _, cx| {
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(InspectorTab::Goals));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowSubagents, _, cx| {
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(InspectorTab::Subagents));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::ShowContext, _, cx| {
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(InspectorTab::Context));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::QueuePrevious, window, cx| {
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
                if this.queue_cursor < this.snapshot.composer.queue.len() {
                    this.dispatch(DesktopIntent::SendQueuedNow {
                        index: this.queue_cursor,
                    });
                }
                this.queue_expanded = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &keymap::SubmitComposer, window, cx| {
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
            .on_action(cx.listener(|this, _: &keymap::GateNext, _, cx| {
                this.move_gate_cursor(1, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::GatePrevious, _, cx| {
                this.move_gate_cursor(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleGateOption, _, cx| {
                this.toggle_gate_cursor(cx);
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
                        v_flex()
                            .id("run-room")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .child(
                                        h_flex()
                                            .size_full()
                                            .min_h_0()
                                            .child(self.render_transcript(window, cx))
                                            .when_some(inline_right, |row, right| row.child(right)),
                                    )
                                    .when(
                                        self.scroller.read(cx).is_scrolled_up()
                                            || !self.snapshot.tail_following,
                                        |region| region.child(self.render_back_to_bottom(cx)),
                                    ),
                            )
                            .when(self.snapshot.failure.is_some(), |column| {
                                column.child(self.render_failure(cx))
                            })
                            .child(self.render_sticky(cx))
                            .child(self.render_composer(window, cx)),
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
                root.child(
                    v_flex()
                        .id("overlay-scrim")
                        .track_focus(&self.overlay_focus)
                        .key_context("Overlay")
                        .absolute()
                        .inset_0()
                        .items_center()
                        .justify_center()
                        .bg(theme::scrim_color())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.close_overlay();
                            this.restore_focus(window, cx);
                            cx.notify();
                        }))
                        .child(
                            v_flex()
                                .id("overlay-content")
                                .on_click(|_, _, cx| cx.stop_propagation())
                                .child(overlay),
                        ),
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
        .rounded_md()
        .px_2()
        .py_0p5()
        .text_xs()
        .when(selected, |button| button.bg(cx.theme().accent))
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
        .gap_2()
        .rounded_md()
        .px_2()
        .py_1()
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
                    .text_xs()
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
                this.right_visible = true;
                this.dispatch(DesktopIntent::SetInspector(tab));
            }
            cx.notify();
        }))
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
        .justify_between()
        .text_sm()
        .child(div().text_color(cx.theme().muted_foreground).child(label))
        .child(div().truncate().child(value.to_string()))
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

fn task_status_label(
    status: tact_desktop_core::TaskStatusSnapshot,
    locale: tact_catalog::CatalogLocale,
) -> &'static str {
    match status {
        tact_desktop_core::TaskStatusSnapshot::Pending => i18n::text(locale, TextKey::Queue),
        tact_desktop_core::TaskStatusSnapshot::InProgress => i18n::text(locale, TextKey::Running),
        tact_desktop_core::TaskStatusSnapshot::Completed => i18n::text(locale, TextKey::Completed),
    }
}

fn cache_rate(usage: &tact_desktop_core::UsageView) -> String {
    let total = usage.cache_hit_tokens.saturating_add(usage.prompt_tokens);
    if total == 0 {
        "0%".into()
    } else {
        format!(
            "{}%",
            (u64::from(usage.cache_hit_tokens) * 100 / u64::from(total)).min(100)
        )
    }
}

fn localized_wait_reason(reason: &str, locale: tact_catalog::CatalogLocale) -> String {
    match (reason, locale) {
        ("paused by user", tact_catalog::CatalogLocale::Zh) => "用户已暂停".into(),
        ("waiting for user", tact_catalog::CatalogLocale::Zh) => "等待用户".into(),
        ("waiting for permission", tact_catalog::CatalogLocale::Zh) => "等待权限确认".into(),
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
            .py_3()
            .child(
                div()
                    .flex_none()
                    .size(px(26.))
                    .rounded_full()
                    .bg(snapshot_row_background(snapshot))
                    .border_1()
                    .border_color(snapshot_row_border(snapshot))
                    .text_xs()
                    .font_semibold()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("Y"),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(snapshot_row_muted(snapshot))
                            .child(i18n::text(snapshot.language, TextKey::You)),
                    )
                    .child(TextView::markdown(
                        format!("user-{id}"),
                        user.text.to_string(),
                    ))
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
            let content = streamed_text(&assistant.markdown, assistant.status, snapshot.language);
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
                        .size(px(26.))
                        .rounded_full()
                        .bg(snapshot_accent(snapshot))
                        .text_color(snapshot_accent_foreground(snapshot))
                        .text_xs()
                        .font_bold()
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
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(snapshot_row_muted(snapshot))
                                .child("Tact"),
                        )
                        .child(body)
                        .when(!assistant.suggestions.is_empty(), |row| {
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
                .gap_1()
                .child(
                    Thinking::new(format!("thinking-{id}"), &progressive)
                        .labels(thinking_labels(snapshot.language))
                        .open(matches!(
                            thinking.status,
                            RowStatus::Running | RowStatus::Pending
                        )),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("open-thinking-{id}")))
                        .role(Role::Button)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(gpui::rgba(0x007aff22)))
                        .aria_label(i18n::text(snapshot.language, TextKey::OpenReader))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(snapshot_row_muted(snapshot))
                        .child(i18n::text(snapshot.language, TextKey::OpenReader))
                        .on_click(move |_, window, cx| {
                            let _ = reader_weak.update(cx, |this, cx| {
                                this.remember_focus(window, cx);
                                this.dispatch(DesktopIntent::OpenReader(ReaderRef::Thinking(
                                    row_id,
                                )));
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
            let tools = group
                .items
                .iter()
                .map(|item| tool_group_item_element(item, row.id, snapshot.language, weak.clone()))
                .collect::<Vec<_>>();
            let group_id = row.id;
            let group_weak = weak.clone();
            div()
                .debug_selector(move || format!("transcript-row-tool-group-{id}"))
                .child(
                    ToolGroup::new(format!("tool-group-{id}"))
                        .title(tool_group_title(group, snapshot.language))
                        .count(tool_count)
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
                                    this.dispatch(DesktopIntent::OpenReader(ReaderRef::Diff(
                                        row_id,
                                    )));
                                });
                            }
                        }),
                )
                .into_any_element()
        }
        RowBody::Gate(gate) => div()
            .debug_selector(move || format!("transcript-row-gate-{id}"))
            .child(gate_element(gate, snapshot.language, weak))
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
            .border_l_2()
            .pl_2()
            .child(
                div()
                    .text_xs()
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
            .when(error.retryable, |card| {
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
    v_flex()
        .id(SharedString::from(format!("row-a11y-{id}")))
        .role(crate::ui::transcript_row::accessibility_role(row))
        .aria_label(crate::ui::transcript_row::accessibility_label(
            row,
            snapshot.language,
        ))
        .child(content)
        .into_any_element()
}

fn tool_group_item_element(
    item: &ToolGroupItem,
    group_id: RowId,
    locale: tact_catalog::CatalogLocale,
    weak: WeakEntity<DesktopRoot>,
) -> AnyElement {
    match item {
        ToolGroupItem::Gate(gate) => gate_element(gate, locale, weak),
        ToolGroupItem::Tool(tool) => tool_row_element(tool, group_id, locale, weak),
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
                        this.dispatch(DesktopIntent::OpenReader(ReaderRef::ToolOutput(tool_id)));
                    });
                }),
        );
    }
    row.into_any_element()
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
    markdown: &Arc<str>,
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
    locale: tact_catalog::CatalogLocale,
    weak: WeakEntity<DesktopRoot>,
) -> AnyElement {
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
        } else if let Some(settled) = settled {
            format!("{} · {settled}", gate.prompt)
        } else {
            gate.prompt.to_string()
        };
        return v_flex()
            .id(SharedString::from(format!(
                "gate-summary-{}",
                gate.request_id.0
            )))
            .role(Role::Status)
            .aria_label(summary.clone())
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(gpui::rgb(0x5f6673))
            .child(div().text_sm().child(summary))
            .child(
                div()
                    .text_xs()
                    .text_color(gpui::rgb(0x98a2b3))
                    .child(origin),
            )
            .into_any_element();
    }
    if matches!(
        gate.purpose,
        tact_desktop_core::UiRequestPurpose::Permission
            | tact_desktop_core::UiRequestPurpose::PlanApproval
    ) {
        let request_id = gate.request_id.0;
        let session_epoch = gate.session_epoch;
        let option_count = gate.options.len();
        let weak = weak.clone();
        return ApprovalCard::new(
            format!("approval-{}", gate.request_id.0),
            gate.prompt.to_string(),
        )
        .description(origin)
        .approve_label(i18n::text(locale, TextKey::Accept))
        .reject_label(i18n::text(locale, TextKey::Cancel))
        .always_label(i18n::text(locale, TextKey::AlwaysAllow))
        .decision(match gate.status {
            GateState::Pending | GateState::Submitting => ApprovalDecision::Pending,
            GateState::Resolved => ApprovalDecision::Approved,
            GateState::Cancelled | GateState::Withdrawn | GateState::Stale => {
                ApprovalDecision::Rejected
            }
        })
        .allow_always(option_count > 2)
        .on_event(move |event, _, cx| {
            let choice = match event {
                ApprovalEvent::Approved { .. } => Some(0),
                ApprovalEvent::Rejected { .. } => Some(1),
                ApprovalEvent::ApprovedAlways { .. } => Some(if option_count > 2 { 2 } else { 0 }),
            };
            let _ = weak.update(cx, |this, _| {
                this.dispatch(DesktopIntent::RespondGate {
                    session_epoch,
                    request_id,
                    response: GateResponse::Select(choice),
                });
            });
        })
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
            QuestionFlow::new(
                format!("gate-{}", gate.request_id.0),
                gate.prompt.to_string(),
            )
            .questions([Question::new("answer", gate.prompt.to_string())
                .note(origin)
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
            .on_event(move |event, _, cx| match event {
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
                        let _ = weak.update(cx, |this, _| {
                            this.dispatch(DesktopIntent::RespondGate {
                                session_epoch,
                                request_id,
                                response: GateResponse::Select(Some(index)),
                            });
                        });
                    }
                }
                QuestionFlowEvent::Skipped { .. } => {
                    let _ = weak.update(cx, |this, _| {
                        this.dispatch(DesktopIntent::RespondGate {
                            session_epoch,
                            request_id,
                            response: GateResponse::Select(None),
                        });
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
            let rows = gate
                .options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let weak = weak.clone();
                    let initial_selected = initial_selected.clone();
                    Toggle::new(format!("gate-option-{}", index), option.to_string())
                        .shape(ToggleShape::Check)
                        .on(selected.contains(&index))
                        .on_event(move |event, _, cx| {
                            let ToggleEvent::Toggled { on, .. } = event;
                            let mut choices = initial_selected.clone();
                            if *on {
                                choices.push(index);
                                choices.sort_unstable();
                                choices.dedup();
                            } else {
                                choices.retain(|candidate| *candidate != index);
                            }
                            let _ = weak.update(cx, |this, _| {
                                this.dispatch(DesktopIntent::SetGateDraft {
                                    session_epoch,
                                    request_id,
                                    selected: choices,
                                });
                            });
                        })
                })
                .collect::<Vec<_>>();
            let confirm_weak = weak;
            let default_choices = initial_selected;
            v_flex()
                .id(SharedString::from(format!(
                    "gate-multi-{}",
                    gate.request_id.0
                )))
                .role(Role::Alert)
                .aria_label(gate.prompt.to_string())
                .gap_2()
                .child(TextView::markdown(
                    format!("gate-multi-prompt-{}", gate.request_id.0),
                    gate.prompt.to_string(),
                ))
                .child(div().text_xs().child(origin))
                .child(div().text_xs().child(match locale {
                    tact_catalog::CatalogLocale::En => {
                        format!("{} selected", selected.len())
                    }
                    tact_catalog::CatalogLocale::Zh => {
                        format!("已选 {} 项", selected.len())
                    }
                }))
                .children(rows)
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            gpui_base::Button::new(format!("gate-multi-cancel-{request_id}"))
                                .role(Role::Button)
                                .tab_stop(true)
                                .aria_label(i18n::text(locale, TextKey::Cancel))
                                .px_2()
                                .py_1()
                                .child(i18n::text(locale, TextKey::Cancel))
                                .on_click({
                                    let weak = confirm_weak.clone();
                                    move |_, _, cx| {
                                        let _ = weak.update(cx, |this, _| {
                                            this.dispatch(DesktopIntent::RespondGate {
                                                session_epoch,
                                                request_id,
                                                response: GateResponse::MultiSelect(None),
                                            });
                                        });
                                    }
                                }),
                        )
                        .child(
                            gpui_base::Button::new(format!("gate-multi-confirm-{request_id}"))
                                .role(Role::Button)
                                .tab_stop(true)
                                .aria_label(i18n::text(locale, TextKey::Confirm))
                                .disabled(default_choices.is_empty())
                                .px_2()
                                .py_1()
                                .child(i18n::text(locale, TextKey::Confirm))
                                .on_click(move |_, _, cx| {
                                    let choices = default_choices.clone();
                                    if choices.is_empty() {
                                        return;
                                    }
                                    let _ = confirm_weak.update(cx, |this, _| {
                                        this.dispatch(DesktopIntent::RespondGate {
                                            session_epoch,
                                            request_id,
                                            response: GateResponse::MultiSelect(Some(
                                                choices.clone(),
                                            )),
                                        });
                                    });
                                }),
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

fn snapshot_accent_foreground(_snapshot: &DesktopSnapshot) -> gpui::Hsla {
    gpui::rgb(0xffffff).into()
}

fn overlay_width(window: &Window, desired: f32) -> gpui::Pixels {
    px((f32::from(window.viewport_size().width) - 24.0).clamp(280.0, desired))
}

fn overlay_height(window: &Window, desired: f32) -> gpui::Pixels {
    px((f32::from(window.viewport_size().height) - 24.0).clamp(180.0, desired))
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
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .px_2()
        .py_0p5()
        .text_xs()
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
    use tact_desktop_core::DesktopConfig;

    use super::*;

    fn install_test_config(config: &DesktopConfig) {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| config.install_process_config().unwrap());
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
            (BuiltinCommand::Hooks, ExtensionsTab::Hooks),
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
        shutdown.shutdown_blocking().unwrap();
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[gpui::test]
    fn gate_close_restores_trigger_focus(cx: &mut TestAppContext) {
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
                this.open_overlay(Overlay::Gate(tact_desktop_core::fixtures::demo_gate()));
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
            "closing a gate must restore the composer trigger focus"
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
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let sticky = window.debug_bounds("sticky-strip").expect("sticky strip");
        let back = window
            .debug_bounds("back-to-bottom")
            .expect("back-to-bottom control");
        assert!(
            back.origin.y + back.size.height <= sticky.origin.y,
            "back={back:?}, sticky={sticky:?}"
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
