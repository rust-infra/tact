//! Local prompt composer built on `gpui-component`'s textarea.

use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Role, SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    popover::Popover,
    v_flex,
};

use crate::ui::leaf::AttachmentItem;
use crate::ui::row_scroll::RowScroll;

const MODEL_MENU_ROW_HEIGHT: f32 = 27.5;
const MODEL_MENU_VISIBLE_ROWS: usize = 8;
const SUGGESTION_ROW_HEIGHT: f32 = 27.5;
const SUGGESTION_VISIBLE_ROWS: usize = 8;

/// The composer's running/ready lifecycle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProgressState {
    #[default]
    Pending,
    Running,
    Complete,
    Cancelled,
    Unknown,
    Failed(String),
}

type ControlBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<AnyElement>>;

/// One selectable model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptModel {
    id: SharedString,
    label: SharedString,
    context_window: Option<u64>,
}

impl PromptModel {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            context_window: None,
        }
    }

    pub fn context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn context_window_tokens(&self) -> Option<u64> {
        self.context_window
    }
}

/// One `@`-mention suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptMention {
    id: SharedString,
    label: SharedString,
}

impl PromptMention {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// One `/` command suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptCommand {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
}

impl PromptCommand {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// User-visible copy for the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptBarLabels {
    pub placeholder: SharedString,
    pub composer: SharedString,
    pub model_prefix: SharedString,
    pub no_models: SharedString,
    pub available_models: SharedString,
    pub search_models: SharedString,
    pub no_matching_models: SharedString,
    pub running: SharedString,
    pub ready: SharedString,
    pub failed_prefix: SharedString,
    pub attachments: SharedString,
    pub suggestions: SharedString,
    pub attach: SharedString,
    pub enhance: SharedString,
    pub cancel: SharedString,
    pub send: SharedString,
}

impl Default for PromptBarLabels {
    fn default() -> Self {
        Self {
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
        }
    }
}

/// The send control's visual shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptSubmit {
    #[default]
    Label,
    Glyph,
    Plane,
}

/// An immutable snapshot of one submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSubmission {
    text: SharedString,
    model_id: Option<SharedString>,
    attachment_ids: Vec<SharedString>,
}

impl PromptSubmission {
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    pub fn model_id(&self) -> Option<&SharedString> {
        self.model_id.as_ref()
    }

    pub fn attachment_ids(&self) -> &[SharedString] {
        &self.attachment_ids
    }
}

/// An interaction emitted by [`PromptBar`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptBarEvent {
    DraftChanged {
        id: SharedString,
        draft: SharedString,
    },
    Submit {
        id: SharedString,
        submission: PromptSubmission,
    },
    CancelRequested {
        id: SharedString,
    },
    ModelChanged {
        id: SharedString,
        model_id: SharedString,
    },
    MentionSelected {
        id: SharedString,
        mention_id: SharedString,
    },
    CommandSelected {
        id: SharedString,
        command_id: SharedString,
    },
    AttachRequested {
        id: SharedString,
    },
    AttachmentRemoved {
        id: SharedString,
        attachment_id: SharedString,
    },
    EnhanceRequested {
        id: SharedString,
        draft: SharedString,
    },
}

/// A controlled composer with a retained native textarea.
pub struct PromptBar {
    id: SharedString,
    editor: Entity<TextareaState>,
    labels: PromptBarLabels,
    models: Vec<PromptModel>,
    selected_model: Option<SharedString>,
    mentions: Vec<PromptMention>,
    commands: Vec<PromptCommand>,
    attachments: Vec<AttachmentItem>,
    progress: ProgressState,
    progress_visible: bool,
    footer_status: Option<SharedString>,
    submit: PromptSubmit,
    attach_visible: bool,
    enhance_visible: bool,
    leading_controls: Option<ControlBuilder>,
    trailing_controls: Option<ControlBuilder>,
    model_menu_open: bool,
    model_menu_row: RowScroll,
    model_query: SharedString,
    model_search: Entity<InputState>,
    suggestions_row: RowScroll,
    last_draft: SharedString,
    _subscription: Subscription,
    _model_search_subscription: Subscription,
}

impl PromptBar {
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let labels = PromptBarLabels::default();
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 5)
                .submit_on_enter(true)
                .placeholder(labels.placeholder.clone())
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                this.on_input_event(event, window, cx);
            },
        );
        let model_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(labels.search_models.clone()));
        let model_search_subscription = cx.subscribe_in(
            &model_search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.model_query = this.model_search.read(cx).value();
                    this.model_menu_row.reset();
                    cx.notify();
                }
            },
        );
        Self {
            id: id.into(),
            editor,
            labels,
            models: Vec::new(),
            selected_model: None,
            mentions: Vec::new(),
            commands: Vec::new(),
            attachments: Vec::new(),
            progress: ProgressState::Pending,
            progress_visible: true,
            footer_status: None,
            submit: PromptSubmit::default(),
            attach_visible: true,
            enhance_visible: true,
            leading_controls: None,
            trailing_controls: None,
            model_menu_open: false,
            model_menu_row: RowScroll::default(),
            model_query: SharedString::default(),
            model_search,
            suggestions_row: RowScroll::default(),
            last_draft: SharedString::default(),
            _subscription: subscription,
            _model_search_subscription: model_search_subscription,
        }
    }

    fn on_input_event(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                let draft = self.editor.read(cx).value();
                if draft != self.last_draft {
                    self.last_draft = draft.clone();
                    self.suggestions_row.reset();
                    cx.emit(PromptBarEvent::DraftChanged {
                        id: self.id.clone(),
                        draft,
                    });
                    cx.notify();
                }
            }
            InputEvent::PressEnter { shift: false, .. } => {
                if let Some(mention) = self.first_visible_mention() {
                    self.insert_mention(&mention, window, cx);
                    return;
                }
                self.emit_submit(cx);
            }
            InputEvent::PressEnter { .. } | InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    fn emit_submit(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let submission = PromptSubmission {
            text: text.into(),
            model_id: self.selected_model.clone(),
            attachment_ids: self
                .attachments
                .iter()
                .map(|attachment| attachment.id().clone())
                .collect(),
        };
        cx.emit(PromptBarEvent::Submit {
            id: self.id.clone(),
            submission,
        });
    }

    fn first_visible_mention(&self) -> Option<PromptMention> {
        let draft = self.last_draft.as_ref();
        let token = mention_token(draft)?;
        self.mentions
            .iter()
            .find(|mention| {
                mention
                    .label()
                    .to_lowercase()
                    .contains(&token.to_lowercase())
                    || mention.id().to_lowercase().contains(&token.to_lowercase())
            })
            .cloned()
    }

    fn insert_mention(
        &mut self,
        mention: &PromptMention,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self.editor.read(cx).value().to_string();
        let Some(token) = mention_token(&draft) else {
            return;
        };
        let start = draft.len() - token.len() - 1;
        let next = format!("{}@{} ", &draft[..start], mention.label());
        self.editor
            .update(cx, |editor, cx| editor.set_value(next, window, cx));
        self.last_draft = self.editor.read(cx).value();
        cx.emit(PromptBarEvent::MentionSelected {
            id: self.id.clone(),
            mention_id: mention.id().clone(),
        });
        cx.notify();
    }

    pub fn set_leading_controls(
        &mut self,
        builder: impl Fn(&mut Window, &mut App) -> Vec<AnyElement> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.leading_controls = Some(Rc::new(builder));
        cx.notify();
    }

    pub fn set_trailing_controls(
        &mut self,
        builder: impl Fn(&mut Window, &mut App) -> Vec<AnyElement> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.trailing_controls = Some(Rc::new(builder));
        cx.notify();
    }

    pub fn set_attach_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.attach_visible != visible {
            self.attach_visible = visible;
            cx.notify();
        }
    }

    pub fn set_enhance_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.enhance_visible != visible {
            self.enhance_visible = visible;
            cx.notify();
        }
    }

    pub fn set_labels(
        &mut self,
        labels: PromptBarLabels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.labels == labels {
            return;
        }
        self.labels = labels;
        self.editor.update(cx, |editor, cx| {
            editor.set_placeholder(self.labels.placeholder.to_string(), window, cx);
        });
        self.model_search.update(cx, |search, cx| {
            search.set_placeholder(self.labels.search_models.to_string(), window, cx);
        });
        cx.notify();
    }

    pub fn set_models(&mut self, models: Vec<PromptModel>, cx: &mut Context<Self>) {
        if self.models == models {
            return;
        }
        self.models = models;
        cx.notify();
    }

    pub fn set_selected_model(&mut self, model: impl Into<SharedString>, cx: &mut Context<Self>) {
        let model = model.into();
        if self.selected_model.as_ref() != Some(&model) {
            self.selected_model = Some(model);
            cx.notify();
        }
    }

    pub fn set_progress(&mut self, progress: ProgressState, cx: &mut Context<Self>) {
        if self.progress != progress {
            self.progress = progress;
            cx.notify();
        }
    }

    pub fn set_progress_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.progress_visible != visible {
            self.progress_visible = visible;
            cx.notify();
        }
    }

    pub fn set_footer_status(&mut self, status: Option<SharedString>, cx: &mut Context<Self>) {
        if self.footer_status != status {
            self.footer_status = status;
            cx.notify();
        }
    }

    pub fn set_submit(&mut self, submit: PromptSubmit, cx: &mut Context<Self>) {
        if self.submit != submit {
            self.submit = submit;
            cx.notify();
        }
    }

    pub fn set_mentions(&mut self, mentions: Vec<PromptMention>, cx: &mut Context<Self>) {
        if self.mentions == mentions {
            return;
        }
        self.mentions = mentions;
        cx.notify();
    }

    pub fn set_commands(&mut self, commands: Vec<PromptCommand>, cx: &mut Context<Self>) {
        if self.commands == commands {
            return;
        }
        self.commands = commands;
        cx.notify();
    }

    pub fn set_attachments(&mut self, attachments: Vec<AttachmentItem>, cx: &mut Context<Self>) {
        if self.attachments == attachments {
            return;
        }
        self.attachments = attachments;
        cx.notify();
    }

    pub fn set_draft(
        &mut self,
        draft: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = draft.into();
        if self.editor.read(cx).value() == draft {
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.set_value(draft.clone(), window, cx));
        self.last_draft = draft;
        cx.notify();
    }

    pub fn draft(&self, cx: &App) -> SharedString {
        self.editor.read(cx).value()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }

    fn suggestions(&self) -> Vec<PromptMention> {
        let Some(token) = mention_token(self.last_draft.as_ref()) else {
            return Vec::new();
        };
        let token = token.to_lowercase();
        self.mentions
            .iter()
            .filter(|mention| {
                mention.label().to_lowercase().contains(&token)
                    || mention.id().to_lowercase().contains(&token)
            })
            .cloned()
            .collect()
    }
}

impl EventEmitter<PromptBarEvent> for PromptBar {}

fn mention_token(draft: &str) -> Option<&str> {
    let at = draft.rfind('@')?;
    let token = &draft[at + 1..];
    (!token.chars().any(char::is_whitespace)).then_some(token)
}

impl Render for PromptBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let leading = self
            .leading_controls
            .as_ref()
            .map(|builder| builder(window, cx))
            .unwrap_or_default();
        let trailing = self
            .trailing_controls
            .as_ref()
            .map(|builder| builder(window, cx))
            .unwrap_or_default();
        let suggestions = self.suggestions();
        let suggestion_count = suggestions.len();
        let suggestion_start = self
            .suggestions_row
            .start(suggestion_count, SUGGESTION_VISIBLE_ROWS);
        let suggestion_window = suggestions
            .iter()
            .skip(suggestion_start)
            .take(SUGGESTION_VISIBLE_ROWS)
            .cloned()
            .collect::<Vec<_>>();
        let suggestions_height =
            SUGGESTION_ROW_HEIGHT * suggestion_count.min(SUGGESTION_VISIBLE_ROWS) as f32;
        let suggestions_row = self.suggestions_row.clone();
        let weak_for_suggestions = cx.weak_entity();
        let running = matches!(self.progress, ProgressState::Running);
        let model_label = self
            .selected_model
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.labels.no_models.to_string());
        let model_query = self.model_query.trim().to_lowercase();
        let all_model_count = self.models.len();
        let show_model_search = all_model_count > MODEL_MENU_VISIBLE_ROWS;
        let model_models = self
            .models
            .iter()
            .filter(|model| {
                model_query.is_empty()
                    || model.id().to_lowercase().contains(&model_query)
                    || model.label().to_lowercase().contains(&model_query)
            })
            .cloned()
            .collect::<Vec<_>>();
        let model_count = model_models.len();
        let model_visible_rows = model_count.min(MODEL_MENU_VISIBLE_ROWS);
        let model_start = self
            .model_menu_row
            .start(model_count, MODEL_MENU_VISIBLE_ROWS);
        let model_window = model_models
            .iter()
            .skip(model_start)
            .take(MODEL_MENU_VISIBLE_ROWS)
            .cloned()
            .collect::<Vec<_>>();
        let model_list_height = MODEL_MENU_ROW_HEIGHT * model_visible_rows as f32;
        let model_menu_height = model_list_height
            + 8.
            + if show_model_search { 34. } else { 0. }
            + if model_count == 0 { 34. } else { 0. };
        let weak_for_menu = cx.weak_entity();
        let open_weak = cx.weak_entity();
        let model_selected = self.selected_model.clone();
        let model_menu_row = self.model_menu_row.clone();
        let model_search = self.model_search.clone();
        let no_models_label = self.labels.no_models.clone();
        let no_matching_models_label = self.labels.no_matching_models.clone();
        let model_control = Popover::new("prompt-bar-model-picker-host")
            .anchor(Anchor::BottomLeft)
            .open(self.model_menu_open)
            .on_open_change(move |open, window, cx| {
                let _ = open_weak.update(cx, |this, cx| {
                    this.model_menu_open = *open;
                    if *open {
                        this.model_query = SharedString::default();
                        this.model_menu_row.reset();
                        this.model_search.update(cx, |search, cx| {
                            search.set_value("", window, cx);
                        });
                    }
                    cx.notify();
                });
            })
            .trigger(
                gpui_base::Button::new("prompt-bar-model-trigger")
                    .debug_selector(|| "prompt-bar-model-trigger".into())
                    .role(Role::Button)
                    .aria_label(self.labels.available_models.clone())
                    .selected(self.model_menu_open)
                    .rounded(px(5.))
                    .px(px(4.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .hover(|style| style.bg(cx.theme().muted))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Icon::new(IconName::Cpu)
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .max_w(px(180.))
                                    .truncate()
                                    .text_xs()
                                    .child(model_label),
                            ),
                    ),
            )
            .content(move |_, _, cx| {
                let weak = weak_for_menu.clone();
                let wheel_scroll = model_menu_row.clone();
                div()
                    .id("prompt-bar-model-picker")
                    .debug_selector(|| "prompt-bar-model-picker".into())
                    .w(px(260.))
                    .h(px(model_menu_height))
                    .max_h(px(270.))
                    .overflow_hidden()
                    .occlude()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .when(show_model_search, |panel| {
                        panel.child(
                            div()
                                .debug_selector(|| "prompt-bar-model-search".into())
                                .px(px(4.))
                                .pt(px(4.))
                                .child(Input::new(&model_search)),
                        )
                    })
                    .child(if model_count == 0 {
                        div()
                            .px_2()
                            .py_2()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(if all_model_count == 0 {
                                no_models_label.clone()
                            } else {
                                no_matching_models_label.clone()
                            })
                            .into_any_element()
                    } else {
                        let rows = model_window
                            .iter()
                            .map(|model| {
                                let model_id = model.id().clone();
                                let selected = model_selected.as_ref() == Some(&model_id);
                                let weak = weak.clone();
                                h_flex()
                                    .id(ElementId::Name(
                                        format!("prompt-bar-model-option-{model_id}").into(),
                                    ))
                                    .debug_selector({
                                        let model_id = model_id.clone();
                                        move || format!("prompt-bar-model-option-{model_id}")
                                    })
                                    .role(Role::Button)
                                    .aria_label(model.label().clone())
                                    .w_full()
                                    .h(px(MODEL_MENU_ROW_HEIGHT))
                                    .items_center()
                                    .gap_2()
                                    .rounded(px(5.))
                                    .px_2()
                                    .py_0()
                                    .when(selected, |row| row.bg(cx.theme().accent))
                                    .hover(|style| style.bg(cx.theme().muted))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .text_xs()
                                            .child(model.label().clone()),
                                    )
                                    .when_some(model.context_window_tokens(), |row, tokens| {
                                        row.child(
                                            div()
                                                .flex_none()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!("{tokens}")),
                                        )
                                    })
                                    .on_click(move |_: &ClickEvent, _, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.selected_model = Some(model_id.clone());
                                            this.model_menu_open = false;
                                            cx.emit(PromptBarEvent::ModelChanged {
                                                id: this.id.clone(),
                                                model_id: model_id.clone(),
                                            });
                                            cx.notify();
                                        });
                                    })
                            })
                            .collect::<Vec<_>>();
                        div()
                            .id("prompt-bar-model-list")
                            .debug_selector(|| "prompt-bar-model-list".into())
                            .w_full()
                            .h(px(model_list_height + 8.))
                            .p_1()
                            .overflow_hidden()
                            .on_scroll_wheel(move |event, _, cx| {
                                let delta: f32 = event.delta.pixel_delta(px(20.)).y.into();
                                if wheel_scroll.wheel(
                                    delta,
                                    model_count,
                                    model_visible_rows,
                                    MODEL_MENU_ROW_HEIGHT,
                                ) {
                                    let weak = weak.clone();
                                    let _ = weak.update(cx, |_, cx| cx.notify());
                                }
                                cx.stop_propagation();
                            })
                            .child(v_flex().w_full().children(rows))
                            .into_any_element()
                    })
            });
        let send = h_flex()
            .id("prompt-bar-send-control")
            .debug_selector(|| "prompt-bar-send-control".into())
            .role(Role::Button)
            .aria_label(self.labels.send.clone())
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(7.))
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .cursor_pointer()
            .child(match self.submit {
                PromptSubmit::Label => self.labels.send.clone().into_any_element(),
                PromptSubmit::Glyph => div().child("↑").into_any_element(),
                PromptSubmit::Plane => div().child("➤").into_any_element(),
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.emit_submit(cx);
            }));
        let stop = h_flex()
            .id("prompt-bar-stop-control")
            .debug_selector(|| "prompt-bar-stop-control".into())
            .role(Role::Button)
            .aria_label(self.labels.cancel.clone())
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(7.))
            .bg(cx.theme().danger)
            .text_color(cx.theme().primary_foreground)
            .cursor_pointer()
            .child("■")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                cx.emit(PromptBarEvent::CancelRequested {
                    id: this.id.clone(),
                });
            }));
        let footer = h_flex()
            .w_full()
            .items_center()
            .gap_1()
            .children(leading)
            .child(div().flex_1())
            .child(model_control)
            .children(trailing)
            .when_some(self.footer_status.clone(), |row, status| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
            })
            .when(running, |row| row.child(stop))
            .child(send);
        let mut attachments = h_flex().flex_wrap().gap_1();
        for attachment in &self.attachments {
            let id = attachment.id().clone();
            attachments = attachments.child(
                h_flex()
                    .id(ElementId::Name(format!("prompt-attachment-{id}").into()))
                    .items_center()
                    .gap_1()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px_2()
                    .py(px(2.))
                    .text_xs()
                    .child(attachment.name().clone())
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("prompt-attachment-remove-{id}").into(),
                            ))
                            .cursor_pointer()
                            .text_color(cx.theme().muted_foreground)
                            .child("×")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.attachments.retain(|item| item.id() != &id);
                                cx.emit(PromptBarEvent::AttachmentRemoved {
                                    id: this.id.clone(),
                                    attachment_id: id.clone(),
                                });
                                cx.notify();
                            })),
                    ),
            );
        }
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Group)
            .aria_label(self.labels.composer.clone())
            .w_full()
            .min_w_0()
            .gap_2()
            .rounded(px(12.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .p_2()
            .when(!self.attachments.is_empty(), |bar| bar.child(attachments))
            .when(!suggestions.is_empty(), |bar| {
                bar.child(
                    div()
                        .id("prompt-bar-suggestions")
                        .debug_selector(|| "prompt-bar-suggestions".into())
                        .occlude()
                        .w_full()
                        .h(px(suggestions_height))
                        .max_h(px(SUGGESTION_ROW_HEIGHT * SUGGESTION_VISIBLE_ROWS as f32))
                        .overflow_hidden()
                        .on_scroll_wheel(move |event, _, cx| {
                            let delta: f32 = event.delta.pixel_delta(px(20.)).y.into();
                            if suggestions_row.wheel(
                                delta,
                                suggestion_count,
                                SUGGESTION_VISIBLE_ROWS,
                                SUGGESTION_ROW_HEIGHT,
                            ) {
                                let weak = weak_for_suggestions.clone();
                                let _ = weak.update(cx, |_, cx| cx.notify());
                            }
                            cx.stop_propagation();
                        })
                        .rounded(px(8.))
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .child(v_flex().children(suggestion_window.iter().enumerate().map(
                            |(offset, mention)| {
                                let index = suggestion_start + offset;
                                h_flex()
                                    .id(ElementId::Name(
                                        format!("prompt-bar-suggestion-{index}").into(),
                                    ))
                                    .debug_selector(move || {
                                        format!("prompt-bar-suggestion-{index}")
                                    })
                                    .role(Role::Button)
                                    .aria_label(mention.label().clone())
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .h(px(SUGGESTION_ROW_HEIGHT))
                                    .px_2()
                                    .py_0()
                                    .hover(|style| style.bg(cx.theme().muted))
                                    .child(
                                        Icon::new(IconName::File)
                                            .xsmall()
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .text_xs()
                                            .child(mention.label().clone()),
                                    )
                                    .on_click(cx.listener({
                                        let mention = mention.clone();
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.insert_mention(&mention, window, cx);
                                        }
                                    }))
                            },
                        ))),
                )
            })
            .child(
                div()
                    .id("prompt-bar-editor")
                    .debug_selector(|| "prompt-bar-editor".into())
                    .w_full()
                    .min_w_0()
                    .h(px(28.))
                    .child(
                        Textarea::new(&self.editor)
                            .h(px(28.))
                            .appearance(false)
                            .bordered(false)
                            .aria_label(self.labels.composer.clone()),
                    ),
            )
            .child(footer)
    }
}
