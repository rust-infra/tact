//! Active-token parsing, filtering, and suggestion retention.

use super::*;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptTokenKind {
    Mention,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PromptToken {
    pub(super) kind: PromptTokenKind,
    pub(super) range: Range<usize>,
    pub(super) query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SuggestionKey {
    Mention(SharedString),
    Command(SharedString),
}

pub(super) fn clipped_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub(super) fn active_prompt_token(draft: &str, cursor: usize) -> Option<PromptToken> {
    let cursor = clipped_char_boundary(draft, cursor);
    let prefix = &draft[..cursor];
    let start = prefix
        .char_indices()
        .rev()
        .find_map(|(offset, character)| {
            character
                .is_whitespace()
                .then_some(offset + character.len_utf8())
        })
        .unwrap_or(0);
    let token = &draft[start..cursor];
    let (kind, query) = if let Some(query) = token.strip_prefix('@') {
        (PromptTokenKind::Mention, query)
    } else {
        let query = token.strip_prefix('/')?;
        (PromptTokenKind::Command, query)
    };
    let end = draft[cursor..]
        .char_indices()
        .find_map(|(offset, character)| character.is_whitespace().then_some(cursor + offset))
        .unwrap_or(draft.len());
    (!query.chars().any(char::is_whitespace)).then(|| PromptToken {
        kind,
        range: start..end,
        query: query.to_owned(),
    })
}

pub(super) fn retain_active_suggestion(
    previous: Option<SuggestionKey>,
    filtered: &[SuggestionKey],
) -> Option<SuggestionKey> {
    previous
        .filter(|candidate| filtered.contains(candidate))
        .or_else(|| filtered.first().cloned())
}

pub(super) fn build_submission(
    draft: &str,
    model_id: Option<SharedString>,
    attachments: &[PromptAttachment],
) -> Option<PromptSubmission> {
    let text = draft.trim();
    (!text.is_empty()).then(|| PromptSubmission {
        text: text.to_owned().into(),
        model_id,
        attachment_ids: attachments.iter().map(|item| item.id().clone()).collect(),
    })
}

impl PromptBar {
    pub(super) fn refresh_suggestions(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        let token = {
            let editor = self.editor.read(cx);
            active_prompt_token(&editor.value(), editor.cursor())
        };
        let filtered: Vec<SuggestionKey> = token
            .as_ref()
            .map(|token| {
                let query = token.query.to_lowercase();
                match token.kind {
                    PromptTokenKind::Mention => self
                        .mentions
                        .iter()
                        .filter(|mention| mention.label.to_lowercase().contains(&query))
                        .map(|mention| SuggestionKey::Mention(mention.id.clone()))
                        .collect(),
                    PromptTokenKind::Command => self
                        .commands
                        .iter()
                        .filter(|command| {
                            command.label.to_lowercase().contains(&query)
                                || command.description.as_ref().is_some_and(|description| {
                                    description.to_lowercase().contains(&query)
                                })
                        })
                        .map(|command| SuggestionKey::Command(command.id.clone()))
                        .collect(),
                }
            })
            .unwrap_or_default();
        let changed = self.token != token || self.filtered != filtered;
        self.token = token;
        self.active_suggestion = retain_active_suggestion(self.active_suggestion.take(), &filtered);
        self.filtered = filtered;
        if changed {
            cx.notify();
        }
        changed
    }

    pub(super) fn on_input_event(
        &mut self,
        editor: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let draft = editor.read(cx).value().to_string();
                self.last_cursor = editor.read(cx).cursor();
                self.refresh_suggestions(cx);
                if self.last_draft != draft {
                    self.last_draft = draft.clone();
                    cx.emit(PromptBarEvent::DraftChanged {
                        id: self.id.clone(),
                        draft: draft.into(),
                    });
                }
            }
            InputEvent::PressEnter { shift: false, .. } => {
                if self.model_menu_open {
                    self.confirm_active_model(window, cx);
                } else if self.token.is_some() && self.active_suggestion.is_some() {
                    self.insert_active_suggestion(window, cx);
                } else {
                    self.submit(window, cx);
                }
                cx.stop_propagation();
            }
            InputEvent::PressEnter { shift: true, .. } | InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    pub(super) fn suggestion_label(&self, key: &SuggestionKey) -> Option<SharedString> {
        match key {
            SuggestionKey::Mention(id) => self
                .mentions
                .iter()
                .find(|mention| &mention.id == id)
                .map(|mention| format!("@{}", mention.label).into()),
            SuggestionKey::Command(id) => self
                .commands
                .iter()
                .find(|command| &command.id == id)
                .map(|command| match &command.description {
                    Some(description) => format!("/{} — {description}", command.label).into(),
                    None => format!("/{}", command.label).into(),
                }),
        }
    }
}
