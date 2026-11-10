//! Thinking card component — the first real [`Component`] implementation.
//!
//! Owns its `ThinkingState`; feeds the shared log through `Ctx` (the
//! placeholder row the card is anchored to); renders active/completed
//! reasoning cards into the provided buffer. Demonstrates the step-9
//! component pattern: state lives in the component, shared surfaces go
//! through `Ctx`, the shell only routes updates/keys/renders.

use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::text::Line;

use crate::{
    Component, Ctx, LogCoordinator,
    i18n::Messages,
    protocol::{AgentUpdate, ThinkingChunk},
    state::{ActiveThinkingBlock, LogItemKind, ThinkingBlock, ThinkingState},
};

/// Streaming reasoning card (thinking → tool → reply flow).
pub struct ThinkingComponent {
    state: ThinkingState,
    messages: Messages,
}

impl ThinkingComponent {
    pub fn new(messages: Messages) -> Self {
        Self {
            state: ThinkingState::default(),
            messages,
        }
    }

    /// Swap the locale. Components own a `Messages` because they write log
    /// text that outlives the frame that produced it, so a language change has
    /// to reach them too — see `App::toggle_language`.
    pub fn set_messages(&mut self, messages: Messages) {
        self.messages = messages;
    }

    pub fn state(&self) -> &ThinkingState {
        &self.state
    }

    /// The log placeholder row this card is anchored to (for the host's
    /// log-scroll cache bookkeeping).
    pub fn anchor_phys_idx(&self) -> Option<usize> {
        self.state
            .active
            .as_ref()
            .map(|a| a.phys_idx)
            .or_else(|| self.state.blocks.last().map(|b| b.phys_idx))
    }

    fn on_thinking_chunk(&mut self, chunk: &ThinkingChunk, log: &mut LogCoordinator) {
        match chunk {
            ThinkingChunk::Started => {
                log.append_blank(LogItemKind::Thinking);
                let phys = log.items.len().saturating_sub(1);
                self.state.active = Some(ActiveThinkingBlock::new(phys, Instant::now()));
            }
            ThinkingChunk::Delta(delta) => {
                if let Some(active) = &mut self.state.active {
                    active.push_delta(delta);
                }
            }
            ThinkingChunk::Finished => {
                if let Some(active) = self.state.active.take()
                    && !active.is_blank()
                {
                    self.state.blocks.push(ThinkingBlock::new(
                        active.block_id.clone(),
                        active.phys_idx,
                        active.content.clone(),
                        active.content.lines().next().unwrap_or("").to_string(),
                        vec![Line::from(active.content.clone())],
                        std::time::Duration::from_millis(120),
                    ));
                }
            }
        }
    }
}

/// Transparent field access: hosts keep `app.<field>…` working after the field
/// type becomes the component (no mechanical churn at call sites).
impl std::ops::Deref for ThinkingComponent {
    type Target = ThinkingState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for ThinkingComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for ThinkingComponent {
    fn on_update(&mut self, update: &AgentUpdate, ctx: &mut Ctx<'_>) -> bool {
        match update {
            AgentUpdate::ThinkingChunk(chunk) => {
                self.on_thinking_chunk(chunk, ctx.log);
                true
            }
            _ => false,
        }
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn priority(&self) -> u8 {
        10
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputMode;
    use crate::PendingQueue;
    use crate::components::tool::ToolEvent;

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        stream_events: &'a mut Vec<crate::state::StreamEvent>,
        tool_events: &'a mut Vec<ToolEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events,
            tool_events,
        }
    }

    #[test]
    fn thinking_start_delta_finish_populates_state_and_log() {
        let mut component =
            ThinkingComponent::new(Messages::by_language(crate::i18n::Language::English));
        let mut log = LogCoordinator::default();
        let mut queue = PendingQueue::default();
        let mut events: Vec<crate::state::StreamEvent> = Vec::new();

        let dirty = component.on_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &mut ctx(&mut log, &mut queue, &mut events, &mut Vec::new()),
        );
        assert!(dirty, "thinking lifecycle always repaints");
        let _ = component.state().active.as_ref().expect("active block");
        assert_eq!(log.items.len(), 1, "placeholder row anchored in the log");

        let dirty = component.on_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("reasoning text".into())),
            &mut ctx(&mut log, &mut queue, &mut events, &mut Vec::new()),
        );
        assert!(dirty);
        assert_eq!(
            component.state().active.as_ref().unwrap().content,
            "reasoning text"
        );

        component.on_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
            &mut ctx(&mut log, &mut queue, &mut events, &mut Vec::new()),
        );
        assert!(component.state().active.is_none());
        assert_eq!(component.state().blocks.len(), 1);
        assert_eq!(component.state().blocks[0].summary, "reasoning text");
    }

    #[test]
    fn unrelated_updates_do_not_repaint() {
        let mut component =
            ThinkingComponent::new(Messages::by_language(crate::i18n::Language::English));
        let mut log = LogCoordinator::default();
        let mut queue = PendingQueue::default();
        let mut events: Vec<crate::state::StreamEvent> = Vec::new();
        let dirty = component.on_update(
            &AgentUpdate::StepAdded(crate::protocol::PlanStep::new(
                "s",
                "t",
                "id",
                std::collections::HashMap::<String, String>::new(),
            )),
            &mut ctx(&mut log, &mut queue, &mut events, &mut Vec::new()),
        );
        assert!(!dirty, "thinking component ignores non-thinking updates");
    }

    /// A finished card lands in the component state with its summary intact.
    /// The host reads that state and paints it; the component draws nothing.
    #[test]
    fn a_finished_card_keeps_its_summary_in_state() {
        let mut component =
            ThinkingComponent::new(Messages::by_language(crate::i18n::Language::English));
        let mut log = LogCoordinator::default();
        let mut queue = PendingQueue::default();
        let mut events: Vec<crate::state::StreamEvent> = Vec::new();
        for chunk in [
            ThinkingChunk::Started,
            ThinkingChunk::Delta("visible summary line".into()),
            ThinkingChunk::Finished,
        ] {
            component.on_update(
                &AgentUpdate::ThinkingChunk(chunk),
                &mut ctx(&mut log, &mut queue, &mut events, &mut Vec::new()),
            );
        }
        let block = component
            .state()
            .blocks
            .last()
            .expect("the finished card is in state");
        assert_eq!(block.summary, "visible summary line");
    }
}
