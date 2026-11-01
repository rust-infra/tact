//! Controlled, virtualized chat transcript composed from gpui-ai primitives.

mod render;
#[cfg(test)]
mod tests;
mod transcript;

use gpui_base::StyledExt as _;
use render::{chat_frame, jump_to_latest_button, transcript_frame};
use transcript::{message_ids_are_unique, structural_splice};

use crate::control::ControlMetricsExt as _;
use crate::scrolling::PolicyScrollbarExt as _;
use crate::{
    attachment::{Attachment, AttachmentEvent, AttachmentStrip},
    control::{outlined_control, outlined_control_with_label},
    cues::{self, Cue},
    motion::{MotionTokens, reveal},
    orbs::Orbs,
    prompt_bar::{PromptBar, PromptBarEvent},
    resolved_layout::ResolvedLayoutKey,
    scrolling::list_scroll_mask,
    stream::{ProgressState, StreamedContent},
    streaming_text::{CitationRef, FollowUp, StreamingText, StreamingTextEvent},
    suggestions::{Suggestion, Suggestions, SuggestionsEvent},
    surface::{icon_button, meta},
    theme::SemanticStyledExt as _,
};
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable as _, FollowMode, FontWeight, InteractiveElement as _, IntoElement as _,
    ListAlignment, ListOffset, ListState, ParentElement as _, Pixels, Render, Role, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, list,
    prelude::FluentBuilder as _, px, relative,
};
use gpui_base::{
    Button,
    motion::{Transition, transition},
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button as LabeledButton, ButtonVariants as _},
    h_flex,
    input::{Escape, InputEvent, Textarea, TextareaState},
    text::TextView,
    v_flex,
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
    time::Duration,
};

/// Where a message sits among its sibling versions (regenerations or edits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchPosition {
    index: usize,
    count: usize,
}

impl BranchPosition {
    /// Creates a position; `index` is zero-based and clamped below `count`
    /// (which is at least one).
    pub fn new(index: usize, count: usize) -> Self {
        let count = count.max(1);
        Self {
            index: index.min(count - 1),
            count,
        }
    }

    /// Zero-based index of the shown version.
    pub fn index(self) -> usize {
        self.index
    }

    /// Number of versions.
    pub fn count(self) -> usize {
        self.count
    }

    /// The accessible name of the version switcher.
    pub fn label(self) -> String {
        format!("Version {} of {}", self.index + 1, self.count)
    }
}

/// The in-place editor for one message, alive only while editing.
struct EditSession {
    message_id: SharedString,
    editor: Entity<TextareaState>,
    _subscription: Subscription,
}

/// How long the copy action shows its "copied" confirmation.
const COPIED_FEEDBACK: Duration = Duration::from_secs(2);

/// The semantic speaker or producer of a [`ChatMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatRole {
    /// A message authored by the person using the application.
    User,
    /// A message authored by the assistant.
    Assistant,
    /// Application or conversation-level guidance.
    System,
    /// Output produced by an application-owned tool invocation.
    Tool,
}

impl ChatRole {
    fn label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Assistant => "Assistant",
            Self::System => "System",
            Self::Tool => "Tool",
        }
    }
}

/// One application-owned message snapshot rendered by [`Chat`].
///
/// IDs must be unique within a controlled snapshot. The message keeps all
/// progressive content and interaction metadata immutable; applications
/// replace the enclosing `Arc<[ChatMessage]>` as work advances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    id: SharedString,
    role: ChatRole,
    author: Option<SharedString>,
    content: StreamedContent,
    citations: Vec<CitationRef>,
    sources: Vec<SharedString>,
    follow_ups: Vec<FollowUp>,
    attachments: Vec<Attachment>,
    branch: Option<BranchPosition>,
    retryable: bool,
    appearance: ChatMessageAppearance,
    actions: Option<MessageActions>,
}

/// Application-controlled presentation for one message.
///
/// Chat deliberately does not prescribe how a message looks: consumers decide
/// alignment and bubble treatment per message (typically by role), so a
/// right-aligned user / left-aligned agent layout is one configuration away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatMessageAppearance {
    alignment: MessageAlignment,
    bubble: MessageBubble,
}

impl Default for ChatMessageAppearance {
    fn default() -> Self {
        Self {
            alignment: MessageAlignment::Leading,
            bubble: MessageBubble::Bordered,
        }
    }
}

/// Horizontal placement of a message within the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAlignment {
    /// Hug the transcript's leading edge (default).
    Leading,
    /// Push toward the transcript's trailing edge.
    Trailing,
}

/// Surface treatment of the message frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageBubble {
    /// Card with border and surface background (default).
    Bordered,
    /// Filled tint with no border (chat-bubble look).
    Filled,
    /// No frame at all; content sits directly on the transcript.
    Plain,
}

impl ChatMessageAppearance {
    /// Creates an appearance with explicit alignment and bubble treatment.
    pub fn new(alignment: MessageAlignment, bubble: MessageBubble) -> Self {
        Self { alignment, bubble }
    }

    /// Horizontal placement of this message.
    pub fn alignment(&self) -> MessageAlignment {
        self.alignment
    }

    /// Surface treatment of this message.
    pub fn bubble(&self) -> MessageBubble {
        self.bubble
    }
}

/// The per-message actions a transcript row offers.
///
/// Actions are quiet at rest and appear on hover or keyboard focus; the last
/// settled message keeps them visible so the most likely next action is one
/// click away. Every action is reported as a typed [`ChatEvent`] — the
/// application performs the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageActions {
    copy: bool,
    regenerate: bool,
    edit: bool,
    feedback: bool,
}

impl MessageActions {
    /// No actions.
    pub fn none() -> Self {
        Self::default()
    }

    /// The conventional set for a role: assistant replies can be copied,
    /// regenerated, and rated; user prompts can be copied and edited; tool
    /// output can be copied; system notes offer nothing.
    pub fn for_role(role: ChatRole) -> Self {
        match role {
            ChatRole::Assistant => Self::none().copy(true).regenerate(true).feedback(true),
            ChatRole::User => Self::none().copy(true).edit(true),
            ChatRole::Tool => Self::none().copy(true),
            ChatRole::System => Self::none(),
        }
    }

    /// Offers copying the message text to the clipboard.
    pub fn copy(mut self, enabled: bool) -> Self {
        self.copy = enabled;
        self
    }

    /// Offers requesting a new response.
    pub fn regenerate(mut self, enabled: bool) -> Self {
        self.regenerate = enabled;
        self
    }

    /// Offers editing (and resending) the message.
    pub fn edit(mut self, enabled: bool) -> Self {
        self.edit = enabled;
        self
    }

    /// Offers helpful / not-helpful feedback.
    pub fn feedback(mut self, enabled: bool) -> Self {
        self.feedback = enabled;
        self
    }

    /// Whether any action is offered.
    pub fn is_empty(&self) -> bool {
        !(self.copy || self.regenerate || self.edit || self.feedback)
    }
}

/// What an empty conversation shows: a welcome, optional guidance, and
/// starter suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatWelcome {
    title: SharedString,
    description: Option<SharedString>,
    suggestions: Vec<Suggestion>,
}

impl ChatWelcome {
    /// Creates a welcome with a headline.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            suggestions: Vec::new(),
        }
    }

    /// Adds supporting guidance under the headline.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds starter prompts, reported through [`ChatEvent::SuggestionSelected`].
    pub fn suggestions(mut self, suggestions: impl IntoIterator<Item = Suggestion>) -> Self {
        self.suggestions = suggestions.into_iter().collect();
        self
    }
}

impl ChatMessage {
    /// Creates a message with stable application identity and progressive text.
    pub fn new(id: impl Into<SharedString>, role: ChatRole, content: StreamedContent) -> Self {
        Self {
            id: id.into(),
            role,
            author: None,
            content,
            citations: Vec::new(),
            sources: Vec::new(),
            follow_ups: Vec::new(),
            attachments: Vec::new(),
            branch: None,
            retryable: false,
            appearance: ChatMessageAppearance::default(),
            actions: None,
        }
    }

    /// Overrides the actions this message offers (default: [`MessageActions::for_role`]).
    pub fn actions(mut self, actions: MessageActions) -> Self {
        self.actions = Some(actions);
        self
    }

    /// The actions this message offers.
    pub fn message_actions(&self) -> MessageActions {
        self.actions
            .unwrap_or_else(|| MessageActions::for_role(self.role))
    }

    /// Sets this message's presentation (alignment and bubble treatment).
    pub fn with_appearance(mut self, appearance: ChatMessageAppearance) -> Self {
        self.appearance = appearance;
        self
    }

    /// This message's presentation.
    pub fn appearance(&self) -> &ChatMessageAppearance {
        &self.appearance
    }

    /// Adds an optional visible author name.
    pub fn author(mut self, author: impl Into<SharedString>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Adds application-owned inline citation metadata.
    pub fn citations(mut self, citations: impl IntoIterator<Item = CitationRef>) -> Self {
        self.citations = citations.into_iter().collect();
        self
    }

    /// Adds source labels shown after progressive content settles.
    pub fn sources(mut self, sources: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.sources = sources.into_iter().map(Into::into).collect();
        self
    }

    /// Adds stable follow-up suggestions.
    pub fn follow_ups(mut self, follow_ups: impl IntoIterator<Item = FollowUp>) -> Self {
        self.follow_ups = follow_ups.into_iter().collect();
        self
    }

    /// Attaches files shown read-only above the content; activating one
    /// reports [`ChatEvent::AttachmentActivated`].
    pub fn attachments(mut self, attachments: impl IntoIterator<Item = Attachment>) -> Self {
        self.attachments = attachments.into_iter().collect();
        self
    }

    /// Marks this message as one of several versions; the header shows a
    /// switcher that reports [`ChatEvent::BranchSelected`].
    pub fn branch(mut self, position: BranchPosition) -> Self {
        self.branch = Some(position);
        self
    }

    /// Controls whether a failed message exposes a retry action.
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Returns the stable application-level message identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the message role.
    pub fn role(&self) -> ChatRole {
        self.role
    }

    /// Returns the optional visible author.
    pub fn author_name(&self) -> Option<&SharedString> {
        self.author.as_ref()
    }

    /// Returns the progressive content snapshot.
    pub fn content(&self) -> &StreamedContent {
        &self.content
    }

    /// Returns inline citation metadata.
    pub fn citation_refs(&self) -> &[CitationRef] {
        &self.citations
    }

    /// Returns source labels.
    pub fn source_labels(&self) -> &[SharedString] {
        &self.sources
    }

    /// Returns follow-up suggestions.
    pub fn follow_up_suggestions(&self) -> &[FollowUp] {
        &self.follow_ups
    }

    /// Returns the attachments carried by this message.
    pub fn attachment_refs(&self) -> &[Attachment] {
        &self.attachments
    }

    /// Returns where this message sits among its versions, if branched.
    pub fn branch_position(&self) -> Option<BranchPosition> {
        self.branch
    }

    /// Returns whether a failed message may be retried.
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    fn accessibility_label(&self) -> SharedString {
        match &self.author {
            Some(author) => format!("{author}, {} message", self.role.label()).into(),
            None => format!("{} message", self.role.label()).into(),
        }
    }

    fn state_description(&self) -> SharedString {
        match self.content.state() {
            ProgressState::Pending => "Pending".into(),
            ProgressState::Running => "Streaming".into(),
            ProgressState::Complete => "Complete".into(),
            ProgressState::Failed(reason) => format!("Failed: {reason}").into(),
        }
    }
}

/// A typed application intent emitted by [`Chat`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    /// A cloned event from the composed [`PromptBar`].
    Prompt(PromptBarEvent),
    /// The user requested another attempt for a failed message.
    RetryRequested {
        /// Stable message identifier.
        message_id: SharedString,
    },
    /// The user selected a follow-up attached to a message.
    FollowUpSelected {
        /// Stable message identifier.
        message_id: SharedString,
        /// Stable follow-up identifier.
        follow_up_id: SharedString,
    },
    /// The user activated a source chip attached to a message.
    SourceActivated {
        /// Stable message identifier.
        message_id: SharedString,
        /// Stable source identifier.
        source_id: SharedString,
        /// The source location.
        url: SharedString,
    },
    /// The user activated an attachment carried by a message.
    AttachmentActivated {
        /// Stable message identifier.
        message_id: SharedString,
        /// Stable attachment identifier.
        attachment_id: SharedString,
    },
    /// The user activated an inline citation attached to a message.
    CitationActivated {
        /// Stable message identifier.
        message_id: SharedString,
        /// Stable citation identifier.
        citation_id: SharedString,
        /// Opaque application-owned destination.
        destination: SharedString,
    },
    /// The user copied a message; the text is already on the clipboard.
    MessageCopied {
        /// Stable message identifier.
        message_id: SharedString,
    },
    /// The user asked for a new response in place of this one.
    RegenerateRequested {
        /// Stable message identifier.
        message_id: SharedString,
    },
    /// The user opened the in-place editor for this message.
    EditRequested {
        /// Stable message identifier.
        message_id: SharedString,
    },
    /// The user committed an in-place edit; the application decides
    /// whether that resends, branches, or simply rewrites the message.
    EditSubmitted {
        /// Stable message identifier.
        message_id: SharedString,
        /// The edited text.
        text: SharedString,
    },
    /// The user abandoned an in-place edit.
    EditCancelled {
        /// Stable message identifier.
        message_id: SharedString,
    },
    /// The user moved to another version of a branched message.
    BranchSelected {
        /// Stable message identifier.
        message_id: SharedString,
        /// Zero-based index of the chosen version.
        index: usize,
    },
    /// The user rated a response.
    FeedbackSubmitted {
        /// Stable message identifier.
        message_id: SharedString,
        /// `true` for helpful, `false` for not helpful.
        positive: bool,
    },
    /// The user chose a welcome suggestion.
    SuggestionSelected {
        /// Stable suggestion identifier.
        suggestion_id: SharedString,
    },
    /// The user returned the transcript to active tail-following.
    JumpedToLatest,
}

/// A controlled virtualized conversation composed with [`PromptBar`] and
/// [`StreamingText`].
///
/// The application owns the `Arc<[ChatMessage]>` snapshot and every async
/// producer. Chat retains only variable-height list, tail-follow, unread, and
/// prompt-subscription state. The pinned `gpui::ListState` is used because it
/// measures wrapped visible rows on demand and preserves logical anchors;
/// `gpui_base::v_virtual_list` requires exact heights for every row up front.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::AppContext;
/// # use std::sync::Arc;
/// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
/// let prompt = cx.new(|cx| PromptBar::new("conversation-prompt", window, cx));
/// let chat = cx.new(|cx| Chat::new("conversation", prompt, window, cx));
/// chat.update(cx, |chat, cx| {
///     chat.set_messages(
///         Arc::from([ChatMessage::new(
///             "welcome",
///             ChatRole::Assistant,
///             StreamedContent::done("How can I help?"),
///         )]),
///         window,
///         cx,
///     );
/// });
/// # }
/// ```
pub struct Chat {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    prompt_bar: Entity<PromptBar>,
    messages: Arc<[ChatMessage]>,
    message_focus_handles: HashMap<SharedString, FocusHandle>,
    list_state: ListState,
    /// Rem size the retained row heights were measured against.
    resolved_layout: ResolvedLayoutKey,
    visible_range: Range<usize>,
    pinned_to_bottom: bool,
    unread_message_ids: HashSet<SharedString>,
    /// Messages appended to a live transcript; they settle in with one
    /// reveal, while a freshly loaded history appears at rest.
    arrivals: HashSet<SharedString>,
    welcome: Option<ChatWelcome>,
    copied_message: Option<SharedString>,
    copied_reset: Option<Task<()>>,
    /// The animated jump-to-latest drive. Owned so exactly one can run;
    /// dropped on user interference, replacement, or completion.
    jump_drive: Option<JumpDrive>,
    jump_generation: u64,
    feedback: HashMap<SharedString, bool>,
    editing: Option<EditSession>,
    _prompt_subscription: Subscription,
}

#[derive(Clone, Copy)]
struct JumpDrive {
    generation: u64,
    distance: Pixels,
    primed: bool,
    progress: f32,
}

impl Chat {
    /// Creates an empty chat around an application-supplied prompt entity.
    pub fn new(
        id: impl Into<SharedString>,
        prompt_bar: Entity<PromptBar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list_state = ListState::new(0, ListAlignment::Bottom, px(256.));
        list_state.set_follow_mode(FollowMode::Tail);
        let weak_chat = cx.weak_entity();
        list_state.set_scroll_handler(move |event, _, cx| {
            weak_chat
                .update(cx, |chat, cx| {
                    chat.visible_range = event.visible_range.clone();
                    chat.pinned_to_bottom = event.is_following_tail;
                    if event.is_following_tail {
                        chat.unread_message_ids.clear();
                    }
                    cx.notify();
                })
                .ok();
        });
        let prompt_subscription = cx.subscribe_in(
            &prompt_bar,
            window,
            |_, _, event: &PromptBarEvent, _, cx| {
                cx.emit(ChatEvent::Prompt(event.clone()));
            },
        );

        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            prompt_bar,
            messages: Arc::from([]),
            message_focus_handles: HashMap::new(),
            list_state,
            resolved_layout: ResolvedLayoutKey::default(),
            visible_range: 0..0,
            pinned_to_bottom: true,
            unread_message_ids: HashSet::new(),
            arrivals: HashSet::new(),
            welcome: None,
            copied_message: None,
            copied_reset: None,
            jump_drive: None,
            jump_generation: 0,
            feedback: HashMap::new(),
            editing: None,
            _prompt_subscription: prompt_subscription,
        }
    }

    /// Sets what an empty conversation shows.
    pub fn set_welcome(&mut self, welcome: Option<ChatWelcome>, cx: &mut Context<Self>) {
        if self.welcome != welcome {
            self.welcome = welcome;
            cx.notify();
        }
    }

    /// Replaces the controlled message snapshot.
    ///
    /// A malformed snapshot containing duplicate IDs is ignored so recycled
    /// rows can never alias one another. Structural edits are applied through
    /// one list splice, while content-only changes invalidate only their
    /// retained rows. When not following the tail, the prior top message ID
    /// and pixel offset are restored in the replacement snapshot.
    pub fn set_messages(
        &mut self,
        messages: Arc<[ChatMessage]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Arc::ptr_eq(&self.messages, &messages) || self.messages.as_ref() == messages.as_ref() {
            return;
        }
        if !message_ids_are_unique(&messages) {
            return;
        }

        let was_following = self.list_state.is_following_tail() && self.pinned_to_bottom;
        let was_empty = self.messages.is_empty();
        let old_offset = self.list_state.logical_scroll_top();
        let old_anchor = self
            .messages
            .get(old_offset.item_ix)
            .map(|message| (message.id.clone(), old_offset.offset_in_item));
        let old_by_id: HashMap<_, _> = self
            .messages
            .iter()
            .map(|message| (message.id.clone(), message))
            .collect();
        let last_retained_index = messages
            .iter()
            .rposition(|message| old_by_id.contains_key(&message.id));
        // A snapshot that shares no identity with the previous one is a
        // different conversation, not a hundred new messages. Without this the
        // switch would cue every message as an arrival, animate them all in,
        // and mark the whole history unread.
        let is_replacement = !was_empty && !messages.is_empty() && last_retained_index.is_none();
        let appended_ids = if is_replacement {
            Vec::new()
        } else {
            messages
                .iter()
                .skip(last_retained_index.map_or(0, |index| index + 1))
                .filter(|message| !old_by_id.contains_key(&message.id))
                .map(|message| message.id.clone())
                .collect::<Vec<_>>()
        };
        // Only messages appended to a live transcript animate in and cue;
        // a freshly loaded history settles silently.
        let arrived = if was_empty {
            Vec::new()
        } else {
            appended_ids.clone()
        };
        let settled = messages
            .iter()
            .filter(|message| {
                old_by_id
                    .get(&message.id)
                    .is_some_and(|old| old.content.is_streaming())
                    && matches!(
                        message.content.state(),
                        ProgressState::Complete | ProgressState::Failed(_)
                    )
            })
            .map(|message| {
                (
                    message.id.clone(),
                    *message.content.state() == ProgressState::Complete,
                )
            })
            .collect::<Vec<_>>();
        let (old_range, new_count) = structural_splice(&self.messages, &messages);
        let structural_new_range = old_range.start..old_range.start + new_count;
        let mut next_focus_handles = HashMap::with_capacity(messages.len());
        for message in messages.iter() {
            let focus_handle = self
                .message_focus_handles
                .remove(&message.id)
                .unwrap_or_else(|| cx.focus_handle());
            next_focus_handles.insert(message.id.clone(), focus_handle);
        }

        if !old_range.is_empty() || new_count > 0 {
            let inserted_focus_handles = messages[structural_new_range.clone()]
                .iter()
                .map(|message| next_focus_handles.get(&message.id).cloned());
            self.list_state
                .splice_focusable(old_range, inserted_focus_handles);
        }
        self.message_focus_handles = next_focus_handles;

        let changed = messages
            .iter()
            .enumerate()
            .filter(|(ix, message)| {
                !structural_new_range.contains(ix)
                    && old_by_id
                        .get(&message.id)
                        .is_some_and(|old| *old != *message)
            })
            .map(|(ix, _)| ix)
            .collect::<Vec<_>>();
        for ix in changed {
            self.list_state.remeasure_items(ix..ix + 1);
        }

        let new_ids = messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<HashSet<_>>();
        self.unread_message_ids
            .retain(|message_id| new_ids.contains(message_id));
        // Transient state belongs to a message, so it cannot outlive one.
        // Otherwise feedback grows without bound across conversation switches
        // and an open editor keeps pointing at a message that is gone.
        self.feedback
            .retain(|message_id, _| new_ids.contains(message_id));
        if self
            .copied_message
            .as_ref()
            .is_some_and(|message_id| !new_ids.contains(message_id))
        {
            self.copied_message = None;
            self.copied_reset = None;
        }
        let stale_editor = self
            .editing
            .as_ref()
            .filter(|session| !new_ids.contains(&session.message_id))
            .map(|session| session.editor.clone());
        if let Some(editor) = stale_editor {
            // Dropping the session is not enough on its own. If the editor
            // being removed holds focus, the window keeps pointing at a
            // handle whose entity is gone: the caret disappears and typing
            // reaches nothing. Hand focus to the composer first.
            let editor_had_focus = editor.read(cx).focus_handle(cx).is_focused(window);
            self.editing = None;
            if editor_had_focus {
                let prompt_focus = self.prompt_bar.read(cx).focus_handle(cx);
                window.focus(&prompt_focus, cx);
            }
        }
        self.messages = messages;
        if self.messages.is_empty() || is_replacement {
            self.jump_drive = None;
        }
        if self.messages.is_empty() {
            self.list_state.set_follow_mode(FollowMode::Tail);
            self.list_state.scroll_to_end();
            self.unread_message_ids.clear();
            self.pinned_to_bottom = true;
        } else if was_following || is_replacement {
            // A different conversation opens at its end, like opening it fresh,
            // rather than inheriting the previous transcript's scroll offset.
            if is_replacement {
                self.list_state.set_follow_mode(FollowMode::Tail);
            }
            self.list_state.scroll_to_end();
            self.unread_message_ids.clear();
            self.pinned_to_bottom = true;
        } else {
            if let Some((anchor_id, offset_in_item)) = old_anchor
                && let Some(item_ix) = self
                    .messages
                    .iter()
                    .position(|message| message.id == anchor_id)
            {
                self.list_state.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item,
                });
            }
            self.unread_message_ids.extend(appended_ids);
        }
        self.arrivals
            .retain(|message_id| new_ids.contains(message_id));
        self.arrivals.extend(arrived.iter().cloned());
        for message_id in arrived {
            cues::emit(cx, Cue::MessageArrived { message_id });
        }
        for (message_id, succeeded) in settled {
            cues::emit(
                cx,
                Cue::ResponseSettled {
                    message_id,
                    succeeded,
                },
            );
        }
        cx.notify();
    }

    /// Returns the current application-owned message snapshot.
    pub fn messages(&self) -> &Arc<[ChatMessage]> {
        &self.messages
    }

    /// Returns the composed prompt entity.
    pub fn prompt_bar(&self) -> &Entity<PromptBar> {
        &self.prompt_bar
    }

    /// Returns the number of messages received while the transcript was offscreen.
    pub fn unread_count(&self) -> usize {
        self.unread_message_ids.len()
    }

    /// Returns whether the list is actively following the latest content.
    pub fn is_pinned_to_bottom(&self) -> bool {
        self.list_state.is_following_tail() && self.pinned_to_bottom
    }

    /// Returns the currently known visible message range.
    pub fn visible_range(&self) -> Range<usize> {
        self.visible_range.clone()
    }

    /// Resumes tail-following, clears unread state, and emits a typed event.
    pub fn scroll_to_latest(&mut self, cx: &mut Context<Self>) {
        self.unread_message_ids.clear();
        cx.emit(ChatEvent::JumpedToLatest);

        // The semantics land immediately; only the travel is staged. Under
        // reduced motion, or with nothing measured to travel through, the
        // jump snaps exactly as it always did.
        let viewport = self.list_state.viewport_bounds().size.height;
        if !crate::motion::motion_is_full(cx)
            || viewport <= gpui::px(0.)
            || self.messages.is_empty()
        {
            self.jump_drive = None;
            self.list_state.set_follow_mode(FollowMode::Tail);
            self.list_state.scroll_to_end();
            self.pinned_to_bottom = true;
            cx.notify();
            return;
        }

        // Distance is measured through the scrollbar readbacks: both walk
        // the list's cached heights synchronously, and unlike per-item
        // bounds they stay readable at the scrolled-to-end sentinel.
        let current = self.list_state.scroll_px_offset_for_scrollbar().y;
        let max = self.list_state.max_offset_for_scrollbar().y;
        if max + current <= gpui::px(1.) {
            // Already at the tail; nothing to travel.
            self.jump_drive = None;
            self.list_state.set_follow_mode(FollowMode::Tail);
            self.list_state.scroll_to_end();
            self.pinned_to_bottom = true;
            cx.notify();
            return;
        }

        // Distance-capped: a tail more than about two viewports out does
        // not scroll a blur of skipped transcript past the reader — the
        // jump starts just under one viewport from the end and settles the
        // remainder, so the motion always ends in legible context.
        let distance = if max + current > viewport * 2.0 {
            // Use the viewport-top coordinate, not scroll_to_end's sentinel
            // one whole viewport past it; subtracting .9V from the sentinel
            // would clamp straight to the tail and show no travel at all.
            self.list_state
                .set_offset_from_scrollbar(gpui::point(Pixels::ZERO, -max + viewport * 0.9));
            viewport * 0.9
        } else {
            max + current
        };
        self.jump_generation = self.jump_generation.wrapping_add(1);
        self.jump_drive = Some(JumpDrive {
            generation: self.jump_generation,
            distance,
            primed: false,
            progress: 0.0,
        });
        self.pinned_to_bottom = false;
        cx.notify();
    }

    fn cancel_jump(&mut self, cx: &mut Context<Self>) {
        if self.jump_drive.take().is_some() {
            cx.notify();
        }
    }

    fn render_jump(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drive) = self.jump_drive else { return };
        // Use the framework's frame demand and transition clock. A hidden
        // chat has no timer waking it; reduction and input apply on the next
        // frame, and every deferred write is guarded by this drive's identity.
        let progress = transition(
            ElementId::Name(format!("{}-jump-{}", self.id, drive.generation).into()),
            if drive.primed || !crate::motion::motion_is_full(cx) {
                1.0_f32
            } else {
                0.0
            },
            Transition::new(MotionTokens::read(cx).standard())
                .ease(crate::motion::ease_in_out_quart),
            window,
            cx,
        );
        cx.defer_in(window, move |chat, _, cx| {
            if !chat
                .jump_drive
                .is_some_and(|active| active.generation == drive.generation)
            {
                return;
            }
            if progress >= 1.0 || !crate::motion::motion_is_full(cx) {
                chat.jump_drive = None;
                chat.list_state.set_follow_mode(FollowMode::Tail);
                chat.list_state.scroll_to_end();
                chat.pinned_to_bottom = true;
                cx.notify();
            } else if !drive.primed {
                chat.jump_drive = Some(JumpDrive {
                    primed: true,
                    ..drive
                });
                cx.notify();
            } else if progress > drive.progress {
                chat.jump_drive = Some(JumpDrive { progress, ..drive });
                let remaining = chat.list_state.max_offset_for_scrollbar().y
                    + chat.list_state.scroll_px_offset_for_scrollbar().y;
                let step = remaining - drive.distance * (1.0 - progress);
                if step > Pixels::ZERO {
                    chat.list_state.scroll_by(step);
                    cx.notify();
                }
            }
        });
    }

    /// Re-measures the transcript after the window's rem size changed.
    ///
    /// Row heights cache wrapped text laid out at the previous rem, and no
    /// message snapshot reports a zoom change, so nothing else invalidates
    /// them. The anchor policy is Chat's own: a transcript already following
    /// the tail keeps following it, and otherwise the message that was first
    /// on screen stays first.
    pub fn begin_edit(
        &mut self,
        message_id: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message_id = message_id.into();
        let Some(message) = self
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            return;
        };
        if self
            .editing
            .as_ref()
            .is_some_and(|session| session.message_id == message_id)
        {
            return;
        }
        let text = message.content.text().to_owned();
        let editor = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true);
            state.set_value(text, window, cx);
            state
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit_edit(window, cx);
                    cx.stop_propagation();
                }
            },
        );
        let focus_handle = editor.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        self.editing = Some(EditSession {
            message_id: message_id.clone(),
            editor,
            _subscription: subscription,
        });
        cx.emit(ChatEvent::EditRequested { message_id });
        cx.notify();
    }

    /// Replaces the in-place editor's draft.
    pub fn set_edit_draft(
        &mut self,
        draft: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = &self.editing else {
            return;
        };
        let draft = draft.into();
        session.editor.update(cx, |editor, cx| {
            editor.set_value(draft, window, cx);
        });
    }

    /// Returns the message being edited in place, if any.
    pub fn editing_message(&self) -> Option<&SharedString> {
        self.editing.as_ref().map(|session| &session.message_id)
    }

    /// Commits the in-place edit. Empty drafts are ignored so a stray Enter
    /// cannot erase a message.
    pub fn submit_edit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.editing else {
            return;
        };
        let draft = session.editor.read(cx).value().to_string();
        let text = draft.trim();
        if text.is_empty() {
            return;
        }
        let message_id = session.message_id.clone();
        let text: SharedString = text.to_owned().into();
        self.editing = None;
        cx.emit(ChatEvent::EditSubmitted { message_id, text });
        cx.notify();
    }

    /// Abandons the in-place edit.
    pub fn cancel_edit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.editing.take() else {
            return;
        };
        cx.emit(ChatEvent::EditCancelled {
            message_id: session.message_id,
        });
        cx.notify();
    }
}

impl EventEmitter<ChatEvent> for Chat {}

impl gpui::Styled for Chat {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for Chat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.render_jump(window, cx);
        // The rem is a resolved-layout input, so a change to it invalidates
        // every measured row height. Reading it here costs nothing; the
        // reaction is deferred so that render itself neither mutates nor
        // notifies.
        let rem_size = window.rem_size();
        if !self.resolved_layout.matches(rem_size) {
            cx.defer_in(window, move |chat, _, cx| {
                chat.resolve_layout(rem_size, cx);
            });
        }

        let tokens = cx.theme().semantic_tokens();
        let transcript_id = ElementId::from((ElementId::from(self.id.clone()), "transcript"));
        let composer_id = ElementId::from((ElementId::from(self.id.clone()), "composer"));
        let unread = self.unread_message_ids.len();
        let show_jump = unread > 0 && !self.list_state.is_following_tail();
        let jump_label: SharedString = format!(
            "Jump to latest, {unread} unread message{}",
            if unread == 1 { "" } else { "s" }
        )
        .into();

        chat_frame(&self.id)
            .capture_any_mouse_down(cx.listener(|chat, _, _, cx| chat.cancel_jump(cx)))
            .capture_key_down(cx.listener(|chat, _, _, cx| chat.cancel_jump(cx)))
            .gap(tokens.spacing.sm)
            .child(
                transcript_frame(transcript_id)
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .child({
                        let chat = cx.weak_entity();
                        let list = self.list_state.clone();
                        gpui::canvas(
                            |_, _, _| (),
                            move |_, _, window, _| {
                                window.on_mouse_event(
                                    move |event: &gpui::ScrollWheelEvent, phase, _, cx| {
                                        if phase == gpui::DispatchPhase::Capture
                                            && list.viewport_bounds().contains(&event.position)
                                        {
                                            chat.update(cx, |chat, cx| chat.cancel_jump(cx)).ok();
                                        }
                                    },
                                );
                            },
                        )
                        .absolute()
                        .size_full()
                    })
                    .when(self.messages.is_empty(), |this| {
                        this.child(self.render_welcome(cx))
                    })
                    .when(!self.messages.is_empty(), |this| {
                        this.child(
                            list(self.list_state.clone(), cx.processor(Self::render_message))
                                .size_full(),
                        )
                        .policy_vertical_scrollbar(&self.list_state, cx)
                        .child(list_scroll_mask(&self.list_state))
                    }),
            )
            .when(show_jump, |this| {
                this.child(
                    jump_to_latest_button(&self.id, jump_label, window, cx)
                        .on_click(cx.listener(|chat, _, _, cx| chat.scroll_to_latest(cx))),
                )
            })
            .child(
                div()
                    .id(composer_id)
                    .debug_selector(|| "chat-composer".into())
                    .role(Role::Group)
                    .aria_label("Message composer")
                    .flex_none()
                    .child(self.prompt_bar.clone()),
            )
            .refine_style(&self.style)
    }
}
