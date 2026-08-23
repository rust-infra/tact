//! Bottom status-bar component: token usage + model metadata.
//!
//! Owns a [`StatusBarState`]; on `TokenUsage` / `ModelInfo` it updates the
//! cached stats the bottom bar renders. Pure field updates — no log/scroll
//! interaction, so the component is self-contained.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx, protocol::AgentUpdate, state::StatusBarState};

pub struct StatusBarComponent {
    state: StatusBarState,
}

impl StatusBarComponent {
    pub fn new(git_branch: impl Into<String>) -> Self {
        Self {
            state: StatusBarState::new(git_branch.into()),
        }
    }

    pub fn state(&self) -> &StatusBarState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut StatusBarState {
        &mut self.state
    }
}

impl Component for StatusBarComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        match update {
            AgentUpdate::TokenUsage(usage) => {
                self.state.token_prompt = usage.prompt;
                self.state.token_completion = usage.completion;
                self.state.token_total = usage.total;
                self.state.token_cache_hit = usage.prompt_cache_hit_tokens;
                self.state.token_cache_miss = usage.prompt_cache_miss_tokens;
                self.state.token_reasoning = usage.reasoning_tokens;
                true
            }
            AgentUpdate::ModelInfo(params) => {
                self.state.model_name = params.model.clone();
                self.state.model_max_tokens = params.max_tokens;
                self.state.model_thinking_budget = params.thinking_budget;
                self.state.model_reasoning_effort = params.reasoning_effort.clone();
                true
            }
            _ => false,
        }
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn render(&self, _area: Rect, _buf: &mut Buffer, _ctx: &Ctx<'_>) -> u16 {
        0
    }

    fn priority(&self) -> u8 {
        30
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
    use crate::{
        InputMode, PendingQueue,
        protocol::{ModelCallParams, TokenUsageInfo},
        state::{LogCoordinator, StreamEvent},
    };

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        events: &'a mut Vec<StreamEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events: events,
        }
    }

    #[test]
    fn token_usage_updates_cache_stats() {
        let mut comp = StatusBarComponent::new("main");
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let usage = TokenUsageInfo {
            prompt: 400,
            completion: 190,
            total: 590,
            prompt_cache_hit_tokens: 50,
            prompt_cache_miss_tokens: 70,
            reasoning_tokens: 20,
        };
        let dirty = comp.on_update(
            &AgentUpdate::TokenUsage(usage),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(dirty);
        assert_eq!(comp.state().token_total, 590);
        assert_eq!(comp.state().token_cache_hit, 50);
        assert_eq!(comp.state().token_reasoning, 20);
    }

    #[test]
    fn model_info_updates_model_metadata() {
        let mut comp = StatusBarComponent::new("main");
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let params = ModelCallParams {
            model: "mock-model".into(),
            max_tokens: 128_000,
            thinking_budget: Some(32_000),
            reasoning_effort: Some("high".into()),
            extra_body: None,
        };
        comp.on_update(
            &AgentUpdate::ModelInfo(params),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert_eq!(comp.state().model_name, "mock-model");
        assert_eq!(comp.state().model_reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn unrelated_updates_do_not_repaint() {
        let mut comp = StatusBarComponent::new("main");
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let dirty = comp.on_update(
            &AgentUpdate::TaskComplete("done".into()),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(!dirty);
    }
}
