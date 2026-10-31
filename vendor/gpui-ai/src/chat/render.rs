//! Message actions and the chat transcript's visual composition.

use super::*;
use crate::ButtonLabelExt as _;

pub(super) fn chat_frame(id: &SharedString) -> Stateful<gpui::Div> {
    v_flex()
        .id(id.clone())
        .accessibility_id(format!("chat.{id}"))
        .role(Role::Log)
        .aria_label("Conversation")
        .tab_group()
        .size_full()
        .min_h_0()
        .min_w_0()
}

pub(super) fn transcript_frame(id: ElementId) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(|| "chat-transcript".into())
        .role(Role::List)
        .aria_label("Messages")
        .min_h_0()
        .min_w_0()
}

pub(super) fn message_frame(chat_id: &SharedString, message: &ChatMessage) -> Stateful<gpui::Div> {
    let id = message.id.clone();
    let debug_id = id.clone();
    v_flex()
        .id((ElementId::from(chat_id.clone()), id.clone()))
        .debug_selector(move || format!("chat-message-{debug_id}"))
        .accessibility_id(format!("chat.message.{id}"))
        .role(Role::ListItem)
        .aria_label(message.accessibility_label())
        .aria_description(message.state_description())
        .w_full()
        .min_w_0()
}

pub(super) fn retry_button(
    chat_id: &SharedString,
    message_id: &SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let debug_id = message_id.clone();
    outlined_control(
        (
            ElementId::from((ElementId::from(chat_id.clone()), message_id.clone())),
            "retry",
        ),
        "Retry message",
        window,
        cx,
    )
    .debug_selector(move || format!("chat-retry-{debug_id}"))
}

pub(super) fn jump_to_latest_button(
    chat_id: &SharedString,
    label: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    outlined_control(
        (ElementId::from(chat_id.clone()), "jump-latest"),
        label,
        window,
        cx,
    )
    .debug_selector(|| "chat-jump-latest".into())
}

impl Chat {
    pub(super) fn forward_streaming_event(
        &mut self,
        message_id: SharedString,
        event: &StreamingTextEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            StreamingTextEvent::FollowUpSelected { id } => {
                cx.emit(ChatEvent::FollowUpSelected {
                    message_id,
                    follow_up_id: id.clone(),
                });
            }
            StreamingTextEvent::CitationActivated { id, destination } => {
                cx.emit(ChatEvent::CitationActivated {
                    message_id,
                    citation_id: id.clone(),
                    destination: destination.clone(),
                });
            }
            StreamingTextEvent::SourceActivated { id, url } => {
                cx.emit(ChatEvent::SourceActivated {
                    message_id,
                    source_id: id.clone(),
                    url: url.clone(),
                });
            }
        }
    }

    fn retry(&mut self, message_id: SharedString, cx: &mut Context<Self>) {
        cx.emit(ChatEvent::RetryRequested { message_id });
    }

    /// Opens the in-place editor for a message, prefilled with its text, and
    /// reports [`ChatEvent::EditRequested`]. Enter or Save reports
    /// [`ChatEvent::EditSubmitted`]; Escape or Cancel reports
    /// [`ChatEvent::EditCancelled`]. The message snapshot is untouched until
    /// the application applies the edit.
    fn copy_message(&mut self, message_id: SharedString, cx: &mut Context<Self>) {
        let Some(message) = self
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(message.content.text().to_owned()));
        self.copied_message = Some(message_id.clone());
        // One-shot confirmation owned by the entity: dropping the chat drops
        // the timer, and a second copy restarts it.
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FEEDBACK).await;
            this.update(cx, |chat, cx| {
                chat.copied_message = None;
                chat.copied_reset = None;
                cx.notify();
            })
            .ok();
        }));
        cx.emit(ChatEvent::MessageCopied { message_id });
        cues::emit(cx, Cue::Copied);
        cx.notify();
    }

    fn submit_feedback(
        &mut self,
        message_id: SharedString,
        positive: bool,
        cx: &mut Context<Self>,
    ) {
        self.feedback.insert(message_id.clone(), positive);
        cx.emit(ChatEvent::FeedbackSubmitted {
            message_id,
            positive,
        });
        cx.notify();
    }

    fn render_actions(
        &self,
        message: &ChatMessage,
        is_last: bool,
        message_focus_handle: &FocusHandle,
        group_name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let actions = message.message_actions();
        if self
            .editing
            .as_ref()
            .is_some_and(|session| session.message_id == message.id)
        {
            return None;
        }
        let settled = matches!(
            message.content.state(),
            ProgressState::Complete | ProgressState::Failed(_)
        );
        if actions.is_empty() || !settled {
            return None;
        }
        let tokens = cx.theme().semantic_tokens();
        let message_id = message.id.clone();
        let base_id = ElementId::from((ElementId::from(self.id.clone()), message_id.clone()));
        let copied = self.copied_message.as_ref() == Some(&message_id);
        let rating = self.feedback.get(&message_id).copied();
        // Quiet at rest; revealed by pointer hover on the row, by keyboard
        // focus inside it, and permanently on the last settled message.
        let always_visible = is_last || message_focus_handle.contains_focused(window, cx);
        let debug_id = message_id.to_string();
        let bar = h_flex()
            .id((base_id.clone(), "actions"))
            .debug_selector(move || format!("chat-actions-{debug_id}"))
            .role(Role::Toolbar)
            .aria_label("Message actions")
            .tab_group()
            .items_center()
            .gap(tokens.spacing.xxs)
            .opacity(if always_visible { 1.0 } else { 0.0 })
            .group_hover(group_name, |style| style.opacity(1.0))
            .when(actions.copy, |bar| {
                let id = message_id.clone();
                let debug_id = message_id.clone();
                let copy_button = icon_button(
                    (base_id.clone(), "copy"),
                    if copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    },
                    if copied { "Copied" } else { "Copy message" },
                    window,
                    cx,
                )
                .debug_selector(move || format!("chat-action-copy-{debug_id}"))
                .when(copied, |button| button.text_color(cx.theme().success))
                .on_click(cx.listener(move |chat, _, _, cx| {
                    chat.copy_message(id.clone(), cx);
                }));
                // The check pops in once per copy: its keyed reveal state is
                // dropped with the icon, so the next copy replays it.
                bar.child(if copied {
                    reveal(copy_button, (base_id.clone(), "copied"), window, cx)
                } else {
                    copy_button
                })
            })
            .when(actions.regenerate, |bar| {
                let id = message_id.clone();
                let debug_id = message_id.clone();
                bar.child(
                    icon_button(
                        (base_id.clone(), "regenerate"),
                        IconName::Redo,
                        "Regenerate response",
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("chat-action-regenerate-{debug_id}"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ChatEvent::RegenerateRequested {
                            message_id: id.clone(),
                        });
                    })),
                )
            })
            .when(actions.edit, |bar| {
                let id = message_id.clone();
                let debug_id = message_id.clone();
                bar.child(
                    icon_button(
                        (base_id.clone(), "edit"),
                        IconName::Replace,
                        "Edit message",
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("chat-action-edit-{debug_id}"))
                    .on_click(cx.listener(move |chat, _, window, cx| {
                        chat.begin_edit(id.clone(), window, cx);
                    })),
                )
            })
            .when(actions.feedback, |bar| {
                let up_id = message_id.clone();
                let down_id = message_id.clone();
                let up_debug_id = message_id.clone();
                let down_debug_id = message_id.clone();
                bar.child(
                    icon_button(
                        (base_id.clone(), "helpful"),
                        IconName::ThumbsUp,
                        if rating == Some(true) {
                            "Marked helpful"
                        } else {
                            "Mark helpful"
                        },
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("chat-action-helpful-{up_debug_id}"))
                    // The selected style paints the shared accent fill; a
                    // second per-site tint would fight it across themes.
                    .selected(rating == Some(true))
                    .on_click(cx.listener(move |chat, _, _, cx| {
                        chat.submit_feedback(up_id.clone(), true, cx);
                    })),
                )
                .child(
                    icon_button(
                        (base_id.clone(), "unhelpful"),
                        IconName::ThumbsDown,
                        if rating == Some(false) {
                            "Marked not helpful"
                        } else {
                            "Mark not helpful"
                        },
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("chat-action-unhelpful-{down_debug_id}"))
                    .selected(rating == Some(false))
                    .on_click(cx.listener(move |chat, _, _, cx| {
                        chat.submit_feedback(down_id.clone(), false, cx);
                    })),
                )
            });
        Some(bar.into_any_element())
    }

    pub(super) fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = cx.theme().semantic_tokens();
        let Some(welcome) = &self.welcome else {
            return div()
                .id((ElementId::from(self.id.clone()), "empty"))
                .role(Role::Status)
                .aria_label("No messages yet")
                .child(crate::surface::empty_state(
                    IconName::Inbox,
                    "No messages yet",
                    Some("Messages appear here as the conversation starts".into()),
                    cx,
                ))
                .into_any_element();
        };
        v_flex()
            .id((ElementId::from(self.id.clone()), "welcome"))
            .debug_selector(|| "chat-welcome".into())
            .role(Role::Group)
            .aria_label(welcome.title.clone())
            .size_full()
            .items_center()
            .justify_center()
            .gap(tokens.spacing.md)
            .p(tokens.spacing.xl)
            .child(Orbs::new())
            .child(
                div()
                    .text_token(tokens.typography.lg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .text_center()
                    .child(welcome.title.clone()),
            )
            .when_some(welcome.description.clone(), |this, description| {
                this.child(
                    div()
                        .max_w(relative(0.8))
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().muted_foreground)
                        .text_center()
                        .child(description),
                )
            })
            .when(!welcome.suggestions.is_empty(), |this| {
                this.child(
                    Suggestions::new((ElementId::from(self.id.clone()), "suggestions"))
                        .items(welcome.suggestions.iter().cloned())
                        .justify_center()
                        .on_event(cx.listener(|_, event: &SuggestionsEvent, _, cx| {
                            let SuggestionsEvent::Selected { id } = event;
                            cx.emit(ChatEvent::SuggestionSelected {
                                suggestion_id: id.clone(),
                            });
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_message(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(message) = self.messages.get(index).cloned() else {
            return div().hidden().into_any_element();
        };
        let tokens = cx.theme().semantic_tokens();
        let message_id = message.id.clone();
        let Some(message_focus_handle) = self.message_focus_handles.get(&message_id).cloned()
        else {
            return div().hidden().into_any_element();
        };
        let content_id = ElementId::from((
            ElementId::from((ElementId::from(self.id.clone()), message_id.clone())),
            "content",
        ));
        let content = match message.role {
            ChatRole::Assistant | ChatRole::Tool => {
                StreamingText::new(content_id, &message.content)
                    .citations(message.citations.clone())
                    .sources(message.sources.clone())
                    .follow_ups(message.follow_ups.clone())
                    .on_event(cx.listener({
                        let message_id = message_id.clone();
                        move |chat, event, _, cx| {
                            chat.forward_streaming_event(message_id.clone(), event, cx);
                        }
                    }))
                    .into_any_element()
            }
            ChatRole::User | ChatRole::System => {
                TextView::markdown(content_id, message.content.text())
                    .selectable(true)
                    .into_any_element()
            }
        };
        let content = match self
            .editing
            .as_ref()
            .filter(|session| session.message_id == message_id)
        {
            Some(session) => self.render_editor(&message_id, session.editor.clone(), window, cx),
            None => content,
        };
        let attachments = (!message.attachments.is_empty()).then(|| {
            let strip_id = ElementId::from((
                ElementId::from((ElementId::from(self.id.clone()), message_id.clone())),
                "attachments",
            ));
            AttachmentStrip::new(strip_id)
                .label("Message attachments")
                .items(message.attachments.iter().cloned())
                .on_event(cx.listener({
                    let message_id = message_id.clone();
                    move |_, event: &AttachmentEvent, _, cx| {
                        if let AttachmentEvent::Opened { id } = event {
                            cx.emit(ChatEvent::AttachmentActivated {
                                message_id: message_id.clone(),
                                attachment_id: id.clone(),
                            });
                        }
                    }
                }))
        });
        let retryable_failure =
            message.retryable && matches!(message.content.state(), ProgressState::Failed(_));
        let author = message
            .author
            .clone()
            .unwrap_or_else(|| message.role.label().into());
        let heading_id = ElementId::from((
            ElementId::from((ElementId::from(self.id.clone()), message_id.clone())),
            "heading",
        ));

        // The semantic list item spans the transcript, while the visual
        // bubble is a constrained child. That separation lets alignment move
        // the actual painted surface instead of merely right-aligning content
        // inside a full-width background.
        let appearance = message.appearance();
        let bubble_debug_id = message_id.clone();
        let group_name: SharedString = format!("{}-message-group-{message_id}", self.id).into();
        let is_last = index + 1 == self.messages.len();
        let actions = self.render_actions(
            &message,
            is_last,
            &message_focus_handle,
            group_name.clone(),
            window,
            cx,
        );
        let row = message_frame(&self.id, &message)
            .track_focus(&message_focus_handle)
            .group(group_name)
            .px(tokens.spacing.md)
            .py(tokens.spacing.sm);
        let bubble = v_flex()
            .id((
                ElementId::from((ElementId::from(self.id.clone()), message_id.clone())),
                "bubble",
            ))
            .debug_selector(move || format!("chat-message-bubble-{bubble_debug_id}"))
            .min_w_0()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.md)
            .py(tokens.spacing.md);
        let bubble = match appearance.bubble() {
            MessageBubble::Bordered => bubble
                .w_auto()
                .max_w(relative(0.82))
                .border_1()
                .border_color(cx.theme().border)
                .rounded(tokens.radius.lg)
                .bg(match message.role {
                    ChatRole::User => cx.theme().secondary,
                    ChatRole::Assistant | ChatRole::System | ChatRole::Tool => {
                        cx.theme().background
                    }
                }),
            MessageBubble::Filled => bubble
                .w_auto()
                .max_w(relative(0.82))
                .rounded(tokens.radius.lg)
                .bg(match message.role {
                    ChatRole::User => cx.theme().secondary,
                    ChatRole::Assistant | ChatRole::System | ChatRole::Tool => cx.theme().muted,
                }),
            MessageBubble::Plain => bubble.w_full(),
        };
        let row = if appearance.alignment() == MessageAlignment::Trailing {
            row.items_end()
        } else {
            row.items_start()
        };

        let arrived = self.arrivals.contains(&message_id);
        let arrival_id = ElementId::from((
            ElementId::from((ElementId::from(self.id.clone()), message_id.clone())),
            "arrival",
        ));
        let heading = div()
            .id(heading_id)
            .role(Role::Heading)
            .aria_label(author.clone())
            .text_token(tokens.typography.sm)
            .text_color(cx.theme().foreground)
            .child(author);
        let header = match message.branch {
            Some(position) => h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(tokens.spacing.sm)
                .child(heading)
                .child(self.render_branch_nav(&message_id, position, window, cx))
                .into_any_element(),
            None => heading.into_any_element(),
        };
        let row = row.child(
            bubble
                .child(header)
                .children(attachments)
                .child(content)
                .when(retryable_failure, |this| {
                    this.child(retry_button(&self.id, &message_id, window, cx).on_click(
                        cx.listener(move |chat, _, _, cx| {
                            chat.retry(message_id.clone(), cx);
                        }),
                    ))
                })
                .children(actions),
        );
        if arrived {
            reveal(row, arrival_id, window, cx).into_any_element()
        } else {
            row.into_any_element()
        }
    }
    fn render_editor(
        &self,
        message_id: &SharedString,
        editor: Entity<TextareaState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = cx.theme().semantic_tokens();
        let base = ElementId::from((ElementId::from(self.id.clone()), message_id.clone()));
        let editor_debug_id = message_id.clone();
        let cancel_debug_id = message_id.clone();
        let save_debug_id = message_id.clone();
        v_flex()
            .id((base.clone(), "editor"))
            .debug_selector(move || format!("chat-edit-editor-{editor_debug_id}"))
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.xs)
            .capture_action(cx.listener(|chat, _: &Escape, window, cx| {
                chat.cancel_edit(window, cx);
            }))
            .child(Textarea::new(&editor))
            .child(
                h_flex()
                    .justify_end()
                    .items_center()
                    .gap(tokens.spacing.xs)
                    .child(
                        outlined_control_with_label(
                            (base.clone(), "edit-cancel"),
                            "Cancel edit",
                            "Cancel",
                            window,
                            cx,
                        )
                        .debug_selector(move || format!("chat-edit-cancel-{cancel_debug_id}"))
                        .on_click(cx.listener(|chat, _, window, cx| {
                            chat.cancel_edit(window, cx);
                        })),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("chat-edit-save-{save_debug_id}"))
                            .child(
                                LabeledButton::new((base, "edit-save"))
                                    .primary()
                                    .small()
                                    .control_metrics(cx)
                                    .text_label("Save")
                                    .on_click(cx.listener(|chat, _, window, cx| {
                                        chat.submit_edit(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_branch_nav(
        &self,
        message_id: &SharedString,
        position: BranchPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = cx.theme().semantic_tokens();
        let base = ElementId::from((ElementId::from(self.id.clone()), message_id.clone()));
        let group_debug_id = message_id.clone();
        let prev_debug_id = message_id.clone();
        let next_debug_id = message_id.clone();
        let prev_id = message_id.clone();
        let next_id = message_id.clone();
        let label: SharedString = position.label().into();
        h_flex()
            .id((base.clone(), "branches"))
            .role(Role::Group)
            .aria_label(label)
            .debug_selector(move || format!("chat-branches-{group_debug_id}"))
            .flex_none()
            .items_center()
            .gap(tokens.spacing.xxs)
            .child(
                icon_button(
                    (base.clone(), "branch-prev"),
                    IconName::ChevronLeft,
                    "Previous version",
                    window,
                    cx,
                )
                .disabled(position.index == 0)
                .debug_selector(move || format!("chat-branch-prev-{prev_debug_id}"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if position.index > 0 {
                        cx.emit(ChatEvent::BranchSelected {
                            message_id: prev_id.clone(),
                            index: position.index - 1,
                        });
                    }
                })),
            )
            .child(meta(
                format!("{} / {}", position.index + 1, position.count),
                cx,
            ))
            .child(
                icon_button(
                    (base, "branch-next"),
                    IconName::ChevronRight,
                    "Next version",
                    window,
                    cx,
                )
                .disabled(position.index + 1 >= position.count)
                .debug_selector(move || format!("chat-branch-next-{next_debug_id}"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if position.index + 1 < position.count {
                        cx.emit(ChatEvent::BranchSelected {
                            message_id: next_id.clone(),
                            index: position.index + 1,
                        });
                    }
                })),
            )
            .into_any_element()
    }
}
