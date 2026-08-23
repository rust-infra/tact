//! Thinking card component — the first real [`Component`] implementation.
//!
//! Owns its `ThinkingState`; feeds the shared log through `Ctx` (the
//! placeholder row the card is anchored to); renders active/completed
//! reasoning cards into the provided buffer. Demonstrates the step-9
//! component pattern: state lives in the component, shared surfaces go
//! through `Ctx`, the shell only routes updates/keys/renders.

use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    Component, Ctx, InputMode, LogCoordinator,
    i18n::Messages,
    protocol::{AgentUpdate, ThinkingChunk},
    state::{ActiveThinkingBlock, LogItemKind, ThinkingBlock, ThinkingState},
    theme::Theme,
};

/// Streaming reasoning card (thinking → tool → reply flow).
pub struct ThinkingComponent {
    state: ThinkingState,
    theme: Theme,
    messages: Messages,
}

impl ThinkingComponent {
    pub fn new(theme: Theme, messages: Messages) -> Self {
        Self {
            state: ThinkingState::default(),
            theme,
            messages,
        }
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
                    self.state.blocks.push(ThinkingBlock {
                        phys_idx: active.phys_idx,
                        content: active.content.clone(),
                        summary: active.content.lines().next().unwrap_or("").to_string(),
                        cached_markdown: vec![Line::from(active.content.clone())],
                        elapsed: std::time::Duration::from_millis(120),
                    });
                }
            }
        }
    }

    fn render_card(&self, block: &ThinkingBlock, area: Rect, buf: &mut Buffer) {
        let title = format!(" 🧠 {} ", self.messages.thinking_title);
        let style = Style::default().fg(self.theme.accent);
        let block_widget = Block::default()
            .borders(Borders::ALL)
            .border_style(style)
            .title(Span::styled(title, style));
        let inner = block_widget.inner(area);
        block_widget.render(area, buf);
        let head = Line::from(Span::styled(
            block.summary.clone(),
            Style::default()
                .fg(self.theme.fg)
                .add_modifier(Modifier::BOLD),
        ));
        Paragraph::new(head).render(inner, buf);
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

    fn render(&self, area: Rect, buf: &mut Buffer, _ctx: &Ctx<'_>) -> u16 {
        // Active card: render its tail; completed cards render their summary.
        let mut used: u16 = 0;
        if let Some(active) = &self.state.active {
            let rows = active.body_line_count().max(1) as u16;
            let card_area = Rect::new(area.x, area.y + used, area.width, rows + 2);
            let block_widget = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.theme.accent))
                .title(Span::styled(
                    " 🧠 live ",
                    Style::default().fg(self.theme.accent),
                ));
            let inner = block_widget.inner(card_area);
            block_widget.render(card_area, buf);
            let text = active.display_tail().join("\n");
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(self.theme.fg),
            )))
            .render(inner, buf);
            used += rows + 2;
        }
        for block in self.state.blocks.iter().rev().take(1) {
            let card_area = Rect::new(area.x, area.y + used, area.width, 3);
            self.render_card(block, card_area, buf);
            used += 3;
        }
        used
    }

    fn priority(&self) -> u8 {
        10
    }
}

/// The component's `InputMode`/`LogCoordinator` imports are unused by the
/// default trait methods; this const keeps the intent explicit for hosts.
#[allow(dead_code)]
const _: Option<(InputMode, LogCoordinator)> = None;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PendingQueue;

    fn ctx<'a>(log: &'a mut LogCoordinator, pending: &'a mut PendingQueue) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
        }
    }

    #[test]
    fn thinking_start_delta_finish_populates_state_and_log() {
        let mut component = ThinkingComponent::new(
            crate::theme::Theme::from(crate::theme::ThemeName::Ink),
            Messages::by_language(crate::i18n::Language::English),
        );
        let mut log = LogCoordinator::default();
        let mut queue = PendingQueue::default();

        let dirty = component.on_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &mut ctx(&mut log, &mut queue),
        );
        assert!(dirty, "thinking lifecycle always repaints");
        let _ = component.state().active.as_ref().expect("active block");
        assert_eq!(log.items.len(), 1, "placeholder row anchored in the log");

        let dirty = component.on_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("reasoning text".into())),
            &mut ctx(&mut log, &mut queue),
        );
        assert!(dirty);
        assert_eq!(
            component.state().active.as_ref().unwrap().content,
            "reasoning text"
        );

        component.on_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
            &mut ctx(&mut log, &mut queue),
        );
        assert!(component.state().active.is_none());
        assert_eq!(component.state().blocks.len(), 1);
        assert_eq!(component.state().blocks[0].summary, "reasoning text");
    }

    #[test]
    fn unrelated_updates_do_not_repaint() {
        let mut component = ThinkingComponent::new(
            crate::theme::Theme::from(crate::theme::ThemeName::Ink),
            Messages::by_language(crate::i18n::Language::English),
        );
        let mut log = LogCoordinator::default();
        let mut queue = PendingQueue::default();
        let dirty = component.on_update(
            &AgentUpdate::StepAdded(crate::protocol::PlanStep::new(
                "s",
                "t",
                "id",
                std::collections::HashMap::<String, String>::new(),
            )),
            &mut ctx(&mut log, &mut queue),
        );
        assert!(!dirty, "thinking component ignores non-thinking updates");
    }

    #[test]
    fn render_draws_card_into_buffer() {
        let mut component = ThinkingComponent::new(
            crate::theme::Theme::from(crate::theme::ThemeName::Ink),
            Messages::by_language(crate::i18n::Language::English),
        );
        let mut log = LogCoordinator::default();
        let mut queue = PendingQueue::default();
        for chunk in [
            ThinkingChunk::Started,
            ThinkingChunk::Delta("visible summary line".into()),
            ThinkingChunk::Finished,
        ] {
            component.on_update(
                &AgentUpdate::ThinkingChunk(chunk),
                &mut ctx(&mut log, &mut queue),
            );
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 5));
        let mut log2 = LogCoordinator::default();
        let mut queue2 = PendingQueue::default();
        let used = component.render(
            Rect::new(0, 0, 60, 5),
            &mut buf,
            &ctx(&mut log2, &mut queue2),
        );
        assert!(used > 0);
        let mut text = String::new();
        for y in 0..5u16 {
            for x in 0..60u16 {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(
            text.contains("visible summary line"),
            "card content should render, got: {text:?}"
        );
    }
}
