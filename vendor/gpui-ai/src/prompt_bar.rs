//! Hybrid-controlled prompt composition with native GPUI text editing.

mod model_picker;
mod suggestions;
#[cfg(test)]
mod tests;

use crate::attachment::{AttachmentEvent, AttachmentStrip};
use crate::context_meter::format_tokens;
use crate::control::composed_button;
use crate::cues::{self, Cue};
use crate::stream::ProgressState;
use crate::surface::{eyebrow, meta};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, Div, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    Render, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled, Subscription,
    Window, deferred, div, prelude::FluentBuilder as _,
};
use gpui_base::StyledExt as _;
use gpui_base::{Align, Button, POPUP_PRIORITY, Positioner};
use gpui_component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, h_flex,
    input::{
        Enter, Escape, InputEvent, MoveDown, MoveEnd, MoveHome, MoveUp, Textarea, TextareaState,
    },
    scroll::ScrollableElement as _,
    v_flex,
};
use std::collections::HashSet;

use model_picker::{prompt_model_control, retain_active_model};
use suggestions::{PromptToken, SuggestionKey, build_submission};

/// A selectable model offered by a [`PromptBar`].
///
/// Beyond its stable ID and label a model may carry a provider (options are
/// grouped by it), a one-line description, and its context window, so the
/// picker reads like a catalog rather than a bare list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptModel {
    id: SharedString,
    label: SharedString,
    provider: Option<SharedString>,
    description: Option<SharedString>,
    context_window: Option<u64>,
    disabled: bool,
}

impl PromptModel {
    /// Creates an enabled model with stable identity and a visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            provider: None,
            description: None,
            context_window: None,
            disabled: false,
        }
    }

    /// Sets the provider the model is grouped under (for example "Anthropic").
    pub fn provider(mut self, provider: impl Into<SharedString>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Adds a one-line description shown under the label.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Records the context window in tokens, shown compactly (`200K`).
    pub fn context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }

    /// Sets whether the model can be selected.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the provider name, when set.
    pub fn provider_name(&self) -> Option<&SharedString> {
        self.provider.as_ref()
    }

    /// Returns the description, when set.
    pub fn description_text(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    /// Returns the context window in tokens, when set.
    pub fn context_window_tokens(&self) -> Option<u64> {
        self.context_window
    }

    /// Returns the stable model identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible model label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// How a composer arranges its action row.
///
/// The row holds a leading cluster — the model, attach, and enhance
/// controls — and the submit control. Where those sit relative to each
/// other is a composition choice an application makes, not something the
/// component should decide for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptActions {
    /// The cluster leads, the submit control sits at the trailing end.
    #[default]
    Split,
    /// Everything gathers at the leading edge.
    Leading,
    /// Everything gathers at the trailing edge.
    Trailing,
}

/// What the composer's submit control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptSubmit {
    /// The words "Send" and "Cancel", in a slot wide enough for both.
    #[default]
    Label,
    /// An arrow, square, at the control's own height — a compact composer
    /// where the affordance is the shape rather than the word.
    Glyph,
}

/// One `@`-mention suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptMention {
    id: SharedString,
    label: SharedString,
}

impl PromptMention {
    /// Creates a mention with stable identity and insertion text.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    /// Returns the stable mention identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible mention label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// One `/` command suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCommand {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
}

impl PromptCommand {
    /// Creates a command with stable identity and insertion text.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    /// Adds secondary text used by the suggestion filter and accessible name.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Returns the stable command identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible command label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// The composer's attachment type: the shared [`Attachment`](crate::attachment::Attachment) so a file keeps
/// one identity, kind, and thumbnail from the composer into the message.
pub use crate::attachment::Attachment as PromptAttachment;

/// User-visible labels for the prompt composer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct PromptBarLabels {
    pub placeholder: SharedString,
    pub composer: SharedString,
    pub model_prefix: SharedString,
    pub no_models: SharedString,
    pub available_models: SharedString,
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

/// A trimmed prompt snapshot emitted for application-owned submission work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSubmission {
    text: SharedString,
    model_id: Option<SharedString>,
    attachment_ids: Vec<SharedString>,
}

impl PromptSubmission {
    /// Returns the submitted prompt text.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// Returns the selected model, when one is configured.
    pub fn model_id(&self) -> Option<&SharedString> {
        self.model_id.as_ref()
    }

    /// Returns stable IDs for the attachments present at submission time.
    pub fn attachment_ids(&self) -> &[SharedString] {
        &self.attachment_ids
    }
}

/// An interaction emitted by [`PromptBar`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptBarEvent {
    /// The native editor draft changed.
    DraftChanged {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Complete current draft.
        draft: SharedString,
    },
    /// The user submitted a non-empty prompt.
    Submit {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Immutable submission snapshot.
        submission: PromptSubmission,
    },
    /// The user requested cancellation of running work.
    CancelRequested {
        /// Stable prompt-bar identifier.
        id: SharedString,
    },
    /// The user selected a model.
    ModelChanged {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Stable selected-model identifier.
        model_id: SharedString,
    },
    /// The user inserted a mention.
    MentionSelected {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Stable selected-mention identifier.
        mention_id: SharedString,
    },
    /// The user inserted a command.
    CommandSelected {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Stable selected-command identifier.
        command_id: SharedString,
    },
    /// The user requested an attachment picker.
    AttachRequested {
        /// Stable prompt-bar identifier.
        id: SharedString,
    },
    /// The user removed an attachment.
    AttachmentRemoved {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Stable removed-attachment identifier.
        attachment_id: SharedString,
    },
    /// The user requested application-owned prompt enhancement.
    EnhanceRequested {
        /// Stable prompt-bar identifier.
        id: SharedString,
        /// Complete draft to enhance.
        draft: SharedString,
    },
}

fn prompt_frame(id: &SharedString, label: SharedString) -> Stateful<Div> {
    v_flex()
        .id(id.clone())
        .role(Role::Group)
        .aria_label(label)
        .w_full()
        .min_w_0()
}

fn prompt_listbox(id: ElementId, label: impl Into<SharedString>) -> Stateful<Div> {
    v_flex().id(id).role(Role::ListBox).aria_label(label)
}

fn prompt_status(id: ElementId, label: SharedString) -> Stateful<Div> {
    div().id(id).role(Role::Status).aria_label(label)
}

fn prompt_control(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &mut App,
) -> Button {
    prompt_control_with_tone(id, label, false, cx)
}

fn prompt_control_with_tone(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    cx: &mut App,
) -> Button {
    let label = label.into();
    prompt_control_shell(id, label.clone(), primary, cx).child(div().child(label))
}

/// The shared control chrome without a label child, for controls that stage
/// their own label slot. `label` is still the accessible name.
fn prompt_control_shell(
    id: impl Into<ElementId>,
    label: SharedString,
    primary: bool,
    cx: &mut App,
) -> Button {
    let tokens = cx.theme().semantic_tokens();
    let (background, foreground, border, hover, active) = if primary {
        (
            cx.theme().button_primary,
            cx.theme().button_primary_foreground,
            cx.theme().primary,
            cx.theme().button_primary_hover,
            cx.theme().button_primary_active,
        )
    } else {
        (
            cx.theme().transparent,
            cx.theme().foreground,
            cx.theme().transparent,
            cx.theme().button_hover,
            cx.theme().button_active,
        )
    };
    composed_button(id, label.clone())
        .flex()
        .items_center()
        .justify_center()
        .h(crate::sizing::control_md(cx))
        .px(tokens.spacing.sm)
        .border_1()
        .border_color(border)
        .rounded(tokens.radius.sm)
        .bg(background)
        .text_token(tokens.typography.sm)
        .text_color(foreground)
        .hover(|style| style.bg(hover))
        .active(|style| style.bg(active))
        .focus_visible(|style| style.border_color(cx.theme().ring))
        .styles(|styles| {
            styles.disabled(|style| {
                style
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().muted_foreground)
            })
        })
}

/// A native, hybrid-controlled prompt composer.
///
/// The entity retains one upstream [`TextareaState`] so IME, selection, cursor,
/// clipboard, and multiline editing remain native. Applications own catalogs,
/// attachments, progress, and every asynchronous operation; the component owns
/// only transient editor, overlay, and focus state.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::AppContext;
/// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
/// let prompt = cx.new(|cx| PromptBar::new("assistant-prompt", window, cx));
/// prompt.update(cx, |prompt, cx| {
///     prompt.set_models([PromptModel::new("fast", "Fast")], cx);
/// });
/// # }
/// ```
pub struct PromptBar {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    editor: Entity<TextareaState>,
    models: Vec<PromptModel>,
    selected_model: Option<SharedString>,
    mentions: Vec<PromptMention>,
    commands: Vec<PromptCommand>,
    attachments: Vec<PromptAttachment>,
    progress: ProgressState,
    last_draft: String,
    last_cursor: usize,
    token: Option<PromptToken>,
    filtered: Vec<SuggestionKey>,
    active_suggestion: Option<SuggestionKey>,
    model_menu_open: bool,
    active_model: Option<SharedString>,
    model_trigger_bounds: Bounds<Pixels>,
    model_trigger_rem_size: Pixels,
    actions: PromptActions,
    submit: PromptSubmit,
    labels: PromptBarLabels,
    _subscriptions: Vec<Subscription>,
}

impl PromptBar {
    /// Creates a prompt composer with one retained native textarea entity.
    pub fn new(
        id: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
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
            |this, editor, event: &InputEvent, window, cx| {
                this.on_input_event(editor, event, window, cx);
            },
        );
        let observation = cx.observe(&editor, |this, editor, cx| {
            let cursor = editor.read(cx).cursor();
            if this.last_cursor != cursor {
                this.last_cursor = cursor;
                this.refresh_suggestions(cx);
            }
        });
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            editor,
            models: Vec::new(),
            selected_model: None,
            mentions: Vec::new(),
            commands: Vec::new(),
            attachments: Vec::new(),
            progress: ProgressState::Pending,
            last_draft: String::new(),
            last_cursor: 0,
            token: None,
            filtered: Vec::new(),
            active_suggestion: None,
            model_menu_open: false,
            active_model: None,
            model_trigger_bounds: Bounds::default(),
            model_trigger_rem_size: window.rem_size(),
            actions: PromptActions::default(),
            submit: PromptSubmit::default(),
            labels,
            _subscriptions: vec![subscription, observation],
        }
    }

    /// Replaces every user-visible label and updates the native placeholder.
    pub fn set_labels(
        &mut self,
        labels: PromptBarLabels,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.labels == labels {
            return;
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_placeholder(labels.placeholder.to_string(), window, cx);
        });
        self.labels = labels;
        cx.notify();
    }

    /// Replaces the model catalog while preserving a still-valid selection.
    ///
    /// A catalog repeating a stable ID is ignored, atomically.
    pub fn set_models(
        &mut self,
        models: impl IntoIterator<Item = PromptModel>,
        cx: &mut gpui::Context<Self>,
    ) {
        let models: Vec<_> = models.into_iter().collect();
        if !stable_ids_are_unique(models.iter().map(PromptModel::id)) {
            return;
        }
        if self.models == models {
            return;
        }
        self.models = models;
        if self.models.is_empty() {
            self.model_menu_open = false;
        }
        if self.selected_model.as_ref().is_none_or(|selected| {
            !self
                .models
                .iter()
                .any(|model| &model.id == selected && !model.disabled)
        }) {
            self.selected_model = self
                .models
                .iter()
                .find(|model| !model.disabled)
                .map(|model| model.id.clone());
        }
        self.active_model = retain_active_model(
            self.active_model.take(),
            self.selected_model.as_ref(),
            &self.models,
        );
        cx.notify();
    }

    /// Applies a valid consumer-controlled model selection.
    pub fn set_selected_model(
        &mut self,
        model_id: impl Into<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) {
        let model_id = model_id.into();
        if self.selected_model.as_ref() == Some(&model_id)
            || !self
                .models
                .iter()
                .any(|model| model.id == model_id && !model.disabled)
        {
            return;
        }
        self.active_model = Some(model_id.clone());
        self.selected_model = Some(model_id);
        cx.notify();
    }

    /// Replaces the `@` mention catalog.
    ///
    /// A catalog repeating a stable ID is ignored, atomically.
    pub fn set_mentions(
        &mut self,
        mentions: impl IntoIterator<Item = PromptMention>,
        cx: &mut gpui::Context<Self>,
    ) {
        let mentions: Vec<_> = mentions.into_iter().collect();
        if !stable_ids_are_unique(mentions.iter().map(PromptMention::id)) {
            return;
        }
        if self.mentions != mentions {
            self.mentions = mentions;
            if !self.refresh_suggestions(cx) {
                cx.notify();
            }
        }
    }

    /// Replaces the `/` command catalog.
    ///
    /// A catalog repeating a stable ID is ignored, atomically.
    pub fn set_commands(
        &mut self,
        commands: impl IntoIterator<Item = PromptCommand>,
        cx: &mut gpui::Context<Self>,
    ) {
        let commands: Vec<_> = commands.into_iter().collect();
        if !stable_ids_are_unique(commands.iter().map(PromptCommand::id)) {
            return;
        }
        if self.commands != commands {
            self.commands = commands;
            if !self.refresh_suggestions(cx) {
                cx.notify();
            }
        }
    }

    /// Replaces application-owned attachments.
    ///
    /// A snapshot repeating a stable ID is ignored, atomically.
    pub fn set_attachments(
        &mut self,
        attachments: impl IntoIterator<Item = PromptAttachment>,
        cx: &mut gpui::Context<Self>,
    ) {
        let attachments: Vec<_> = attachments.into_iter().collect();
        if !stable_ids_are_unique(attachments.iter().map(PromptAttachment::id)) {
            return;
        }
        if self.attachments != attachments {
            self.attachments = attachments;
            cx.notify();
        }
    }

    /// Chooses how the action row arranges its controls.
    ///
    /// The default splits them: the model, attach, and enhance cluster
    /// leads, and submit sits at the trailing end of the row.
    pub fn set_actions(&mut self, actions: PromptActions, cx: &mut gpui::Context<Self>) {
        if self.actions != actions {
            self.actions = actions;
            cx.notify();
        }
    }

    /// Chooses whether the submit control reads as a word or a glyph.
    pub fn set_submit(&mut self, submit: PromptSubmit, cx: &mut gpui::Context<Self>) {
        if self.submit != submit {
            self.submit = submit;
            cx.notify();
        }
    }

    /// Returns how the action row arranges its controls.
    pub fn actions(&self) -> PromptActions {
        self.actions
    }

    /// Returns what the submit control shows.
    pub fn submit_appearance(&self) -> PromptSubmit {
        self.submit
    }

    /// Replaces the application-owned progress snapshot.
    pub fn set_progress(&mut self, progress: ProgressState, cx: &mut gpui::Context<Self>) {
        if self.progress != progress {
            self.progress = progress;
            cx.notify();
        }
    }

    /// Replaces the native editor draft, placing the caret at its UTF-8 byte end
    /// without rebuilding the editor entity.
    pub fn set_draft(
        &mut self,
        draft: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let draft = draft.into().to_string();
        if self.last_draft == draft {
            return;
        }
        self.last_draft = draft.clone();
        let cursor = draft.len();
        self.last_cursor = cursor;
        self.editor.update(cx, |editor, cx| {
            editor.set_value(draft, window, cx);
            editor.set_selected_range(cursor..cursor, cx);
        });
        if !self.refresh_suggestions(cx) {
            cx.notify();
        }
    }

    /// Returns the complete current native-editor draft.
    pub fn draft(&self, cx: &App) -> SharedString {
        self.editor.read(cx).value().to_string().into()
    }

    /// Moves keyboard focus into the retained native textarea.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle(cx).focus(window, cx);
    }

    fn move_active_suggestion(&mut self, direction: isize, cx: &mut gpui::Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let current = self
            .active_suggestion
            .as_ref()
            .and_then(|active| {
                self.filtered
                    .iter()
                    .position(|candidate| candidate == active)
            })
            .unwrap_or(0);
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(self.filtered.len() - 1)
        } else {
            (current + 1) % self.filtered.len()
        };
        self.active_suggestion = self.filtered.get(next).cloned();
        cx.notify();
    }

    fn capture_suggestion_action(
        &mut self,
        direction: Option<isize>,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.filtered.is_empty() {
            return;
        }
        match direction {
            Some(direction) => self.move_active_suggestion(direction, cx),
            None => {
                self.token = None;
                self.filtered.clear();
                self.active_suggestion = None;
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn capture_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.model_menu_open {
            self.move_active_model(-1, cx);
            cx.stop_propagation();
        } else {
            self.capture_suggestion_action(Some(-1), cx);
        }
    }

    fn capture_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.model_menu_open {
            self.move_active_model(1, cx);
            cx.stop_propagation();
        } else {
            self.capture_suggestion_action(Some(1), cx);
        }
    }

    fn capture_move_home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.model_menu_open {
            self.move_active_model_to_edge(false, cx);
            cx.stop_propagation();
        }
    }

    fn capture_move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.model_menu_open {
            self.move_active_model_to_edge(true, cx);
            cx.stop_propagation();
        }
    }

    fn capture_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let handled = if self.model_menu_open {
            self.model_menu_open = false;
            true
        } else if self.token.is_some() {
            self.token = None;
            self.filtered.clear();
            self.active_suggestion = None;
            true
        } else {
            false
        };
        if handled {
            self.focus(window, cx);
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn capture_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if action.shift {
            return;
        }
        if self.model_menu_open {
            self.confirm_active_model(window, cx);
        } else if self.token.is_some() && self.active_suggestion.is_some() {
            self.insert_active_suggestion(window, cx);
        } else {
            self.submit(window, cx);
        }
        cx.stop_propagation();
    }

    fn insert_active_suggestion(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let (Some(token), Some(active)) = (self.token.clone(), self.active_suggestion.clone())
        else {
            return;
        };
        let separator = {
            let draft = self.editor.read(cx).value();
            if draft
                .get(token.range.end..)
                .and_then(|suffix| suffix.chars().next())
                .is_none_or(|character| !character.is_whitespace())
            {
                " "
            } else {
                ""
            }
        };
        let (replacement, event) = match active {
            SuggestionKey::Mention(id) => {
                let Some(mention) = self.mentions.iter().find(|mention| mention.id == id) else {
                    return;
                };
                (
                    format!("@{}{separator}", mention.label),
                    PromptBarEvent::MentionSelected {
                        id: self.id.clone(),
                        mention_id: mention.id.clone(),
                    },
                )
            }
            SuggestionKey::Command(id) => {
                let Some(command) = self.commands.iter().find(|command| command.id == id) else {
                    return;
                };
                (
                    format!("/{}{separator}", command.label),
                    PromptBarEvent::CommandSelected {
                        id: self.id.clone(),
                        command_id: command.id.clone(),
                    },
                )
            }
        };
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(token.range, cx);
            editor.replace(&replacement, window, cx);
        });
        self.token = None;
        self.filtered.clear();
        self.active_suggestion = None;
        cx.emit(event);
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.progress == ProgressState::Running {
            return;
        }
        let draft = self.editor.read(cx).value().to_string();
        let Some(submission) =
            build_submission(&draft, self.selected_model.clone(), &self.attachments)
        else {
            return;
        };
        cx.emit(PromptBarEvent::Submit {
            id: self.id.clone(),
            submission,
        });
        cues::emit(cx, Cue::Submitted);
        self.last_draft.clear();
        self.editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
        });
        self.token = None;
        self.filtered.clear();
        self.active_suggestion = None;
        cx.notify();
    }
}

impl EventEmitter<PromptBarEvent> for PromptBar {}

impl Focusable for PromptBar {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl gpui::Styled for PromptBar {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for PromptBar {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let labels = self.labels.clone();
        let tokens = cx.theme().semantic_tokens();
        let root_id = self.id.clone();
        let suggestions = self
            .filtered
            .iter()
            .filter_map(|key| {
                let label = self.suggestion_label(key)?;
                let key = key.clone();
                let selected = self.active_suggestion.as_ref() == Some(&key);
                Some(
                    prompt_control(
                        (
                            gpui::ElementId::from(root_id.clone()),
                            format!("suggestion-{key:?}"),
                        ),
                        label.clone(),
                        cx,
                    )
                    .role(Role::ListBoxOption)
                    .selected(selected)
                    .aria_selected(selected)
                    .w_full()
                    .when(selected, |button| button.bg(cx.theme().accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active_suggestion = Some(key.clone());
                        this.insert_active_suggestion(window, cx);
                    })),
                )
            })
            .collect::<Vec<_>>();
        let attachments = self.attachments.clone();
        let selected_model_label: SharedString = self
            .selected_model
            .as_ref()
            .and_then(|selected| self.models.iter().find(|model| &model.id == selected))
            .map(|model| format!("{}{}", labels.model_prefix, model.label).into())
            .unwrap_or_else(|| labels.no_models.clone());
        let draft = self.editor.read(cx).value().to_string();
        let running = self.progress == ProgressState::Running;
        // Send and Cancel share one slot; the face the control settles into
        // fades in once per state, and the state it mounts with is exempt.
        let submit_ack = crate::motion::acknowledged_state(
            ElementId::from((ElementId::from(self.id.clone()), "submit-swap")),
            running as u64,
            window,
            cx,
        );
        let progress_text = match &self.progress {
            ProgressState::Pending => None,
            ProgressState::Running => Some(labels.running.clone()),
            ProgressState::Complete => Some(labels.ready.clone()),
            ProgressState::Failed(reason) => {
                Some(format!("{}{reason}", labels.failed_prefix).into())
            }
        };

        prompt_frame(&self.id, labels.composer.clone())
            .gap(tokens.spacing.sm)
            .p(tokens.spacing.md)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(tokens.radius.lg)
            .bg(tokens.colors.surface)
            .capture_action(cx.listener(Self::capture_enter))
            .capture_action(cx.listener(Self::capture_move_up))
            .capture_action(cx.listener(Self::capture_move_down))
            .capture_action(cx.listener(Self::capture_move_home))
            .capture_action(cx.listener(Self::capture_move_end))
            .capture_action(cx.listener(Self::capture_escape))
            .when(!attachments.is_empty(), |this| {
                this.child(
                    AttachmentStrip::new((gpui::ElementId::from(root_id.clone()), "attachments"))
                        .label(labels.attachments.clone())
                        .items(attachments)
                        .removable(true)
                        .compact(true)
                        .on_event(cx.listener(|this, event: &AttachmentEvent, _, cx| {
                            if let AttachmentEvent::Removed { id } = event {
                                cx.emit(PromptBarEvent::AttachmentRemoved {
                                    id: this.id.clone(),
                                    attachment_id: id.clone(),
                                });
                            }
                        })),
                )
            })
            .child(
                div()
                    .debug_selector(|| "prompt-bar-editor".to_owned())
                    .w_full()
                    .child(Textarea::new(&self.editor)),
            )
            .when(!suggestions.is_empty(), |this| {
                this.child(
                    crate::popup::popover_surface(
                        prompt_listbox(
                            (gpui::ElementId::from(root_id.clone()), "suggestions").into(),
                            labels.suggestions.clone(),
                        ),
                        cx,
                    )
                    .debug_selector(|| "prompt-bar-suggestions".to_owned())
                    // A list of mentions and commands is a panel over the
                    // composer, not part of it. It carried no surface at
                    // all, so on a dark theme it was invisible: nothing
                    // said anything had opened.
                    .p(tokens.spacing.xs)
                    .max_h(tokens.spacing.xxl + tokens.spacing.xxl + tokens.spacing.xxl)
                    .overflow_y_scrollbar()
                    .children(suggestions),
                )
            })
            .when_some(progress_text, |this, progress_text| {
                this.child(
                    prompt_status(
                        (gpui::ElementId::from(root_id.clone()), "progress").into(),
                        progress_text.clone(),
                    )
                    .text_token(tokens.typography.xs)
                    .text_color(cx.theme().muted_foreground)
                    .child(progress_text),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens.spacing.xs)
                    // The arrangement is the application's: a split row
                    // leads with the cluster and ends with submit, and the
                    // gathered arrangements put everything on one side.
                    .map(|row| match self.actions {
                        PromptActions::Split => row.justify_between(),
                        PromptActions::Leading => row.justify_start(),
                        PromptActions::Trailing => row.justify_end(),
                    })
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap(tokens.spacing.xs)
                            .child(if self.models.is_empty() {
                                prompt_status(
                                    (gpui::ElementId::from(root_id.clone()), "model-empty").into(),
                                    selected_model_label.clone(),
                                )
                                .debug_selector(|| "prompt-bar-model-empty".to_owned())
                                .text_token(tokens.typography.sm)
                                .text_color(cx.theme().muted_foreground)
                                .child(selected_model_label)
                                .into_any_element()
                            } else {
                                let prompt = cx.entity();
                                div()
                                    .on_prepaint(move |bounds, window, cx| {
                                        let rem_size = window.rem_size();
                                        let changed = prompt.update(cx, |prompt, _| {
                                            let changed = prompt.model_trigger_bounds != bounds
                                                || prompt.model_trigger_rem_size != rem_size;
                                            prompt.model_trigger_bounds = bounds;
                                            prompt.model_trigger_rem_size = rem_size;
                                            changed
                                        });
                                        if changed {
                                            window.request_animation_frame();
                                        }
                                    })
                                    .child(
                                        prompt_model_control(
                                            (gpui::ElementId::from(root_id.clone()), "model"),
                                            selected_model_label,
                                            self.model_menu_open,
                                            cx,
                                        )
                                        .debug_selector(|| "prompt-bar-model-trigger".to_owned())
                                        .border_color(cx.theme().border)
                                        .when(self.model_menu_open, |button| {
                                            button.bg(cx.theme().accent)
                                        })
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.toggle_model_menu(window, cx);
                                            }),
                                        ),
                                    )
                                    .into_any_element()
                            })
                            .child(
                                prompt_control(
                                    (gpui::ElementId::from(root_id.clone()), "attach"),
                                    labels.attach.clone(),
                                    cx,
                                )
                                .debug_selector(|| "prompt-bar-attach-control".to_owned())
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.emit(PromptBarEvent::AttachRequested {
                                            id: this.id.clone(),
                                        });
                                    },
                                )),
                            )
                            .child(
                                prompt_control(
                                    (gpui::ElementId::from(root_id.clone()), "enhance"),
                                    labels.enhance.clone(),
                                    cx,
                                )
                                .debug_selector(|| "prompt-bar-enhance-control".to_owned())
                                .disabled(draft.trim().is_empty() || running)
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        let draft = this.editor.read(cx).value().to_string();
                                        cx.emit(PromptBarEvent::EnhanceRequested {
                                            id: this.id.clone(),
                                            draft: draft.into(),
                                        });
                                    },
                                )),
                            ),
                    )
                    .child(
                        prompt_control_shell(
                            (gpui::ElementId::from(root_id.clone()), "submit"),
                            if running {
                                labels.cancel.clone()
                            } else {
                                labels.send.clone()
                            },
                            true,
                            cx,
                        )
                        .map(|button| match self.submit {
                            // A glyph composer says what it does with a
                            // shape: one square control at its own height,
                            // an arrow to send and a square to cancel.
                            PromptSubmit::Glyph => button
                                .w(crate::sizing::control_lg(cx))
                                .px(gpui::Pixels::ZERO)
                                .justify_center()
                                .rounded(cx.theme().radius_full())
                                .child(
                                    div().opacity(submit_ack).child(
                                        Icon::new(if running {
                                            IconName::Close
                                        } else {
                                            IconName::ArrowUp
                                        })
                                        .small(),
                                    ),
                                ),
                            // Zero-height ghosts of both faces hold the slot
                            // at the widest label, so Send↔Cancel swaps
                            // without nudging the composer row.
                            PromptSubmit::Label => button.child(
                                v_flex()
                                    .relative()
                                    .items_center()
                                    .children([labels.send.clone(), labels.cancel.clone()].map(|ghost| {
                                        div()
                                            .h(gpui::rems(0.))
                                            .overflow_hidden()
                                            .opacity(0.)
                                            .child(ghost)
                                    }))
                                    .child(div().opacity(submit_ack).child(if running {
                                        labels.cancel.clone()
                                    } else {
                                        labels.send.clone()
                                    })),
                            ),
                        })
                        .when(running, |button| {
                            button.debug_selector(|| "prompt-bar-cancel-control".to_owned())
                        })
                        .when(!running, |button| {
                            button.debug_selector(|| "prompt-bar-send-control".to_owned())
                        })
                        .disabled(!running && draft.trim().is_empty())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if running {
                                    cx.emit(PromptBarEvent::CancelRequested {
                                        id: this.id.clone(),
                                    });
                                    cues::emit(cx, Cue::Cancelled);
                                } else {
                                    this.submit(window, cx);
                                }
                            },
                        )),
                    ),
            )
            .when(self.model_menu_open, |this| {
                this.child(self.render_model_picker(
                    &root_id,
                    labels.available_models.clone(),
                    window,
                    cx,
                ))
            })
            .refine_style(&self.style)
    }
}

/// Rejects a catalog that repeats a stable ID.
///
/// Repeated IDs alias `ElementId`s, so a second entry would silently take the
/// first one's focus, hover, and reveal state. [`Chat`](crate::chat::Chat) and
/// [`RecordsTable`](crate::records_table::RecordsTable) reject malformed
/// controlled snapshots the same way.
fn stable_ids_are_unique<'a>(mut ids: impl Iterator<Item = &'a SharedString>) -> bool {
    let mut seen = HashSet::new();
    ids.all(|id| seen.insert(id))
}
