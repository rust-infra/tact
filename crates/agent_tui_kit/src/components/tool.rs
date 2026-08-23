//! Tool-activity component: live output + subagent metadata.
//!
//! Owns a [`ToolState`]; on `ToolProgress` it feeds the active card's
//! `live_output` buffer, on `ToolMeta` it updates subagent model/tokens.
//! The `Step*` lifecycle (placeholder-row allocation, finalization, scroll)
//! is entangled with the host log and stays in the shell — a future slice
//! extracts it behind a `ToolEvent` outbox, mirroring `StreamComponent`.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{
    Component, Ctx,
    protocol::{AgentUpdate, TokenUsageInfo, ToolOutputChunk},
    state::{ActiveToolBlock, ToolState},
};

pub struct ToolComponent {
    state: ToolState,
}

impl ToolComponent {
    pub fn new() -> Self {
        Self {
            state: ToolState::default(),
        }
    }

    pub fn state(&self) -> &ToolState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ToolState {
        &mut self.state
    }

    /// Record an active tool (the shell calls this when it applies a
    /// `StepStarted` — the placeholder `phys_idx` is shell-assigned).
    pub fn begin_active(
        &mut self,
        phys_idx: usize,
        tool_id: String,
        output: crate::widgets::tool_widget::ToolRenderOutput,
        live_output: crate::protocol::ToolOutputBuffer,
    ) {
        self.state.active.push(ActiveToolBlock {
            phys_idx,
            tool_id,
            output,
            live_output,
            started_at: std::time::Instant::now(),
        });
    }

    /// Finish an active tool (the shell calls this after finalizing the card).
    pub fn finish_active(
        &mut self,
        tool_id: &str,
        output: crate::widgets::tool_widget::ToolRenderOutput,
    ) {
        self.state.active.retain(|a| a.tool_id != tool_id);
        self.state.blocks.push(crate::state::ToolBlock {
            phys_idx: output.visual_rows(false).saturating_sub(1),
            tool_id: tool_id.to_string(),
            output,
        });
    }
}

impl Default for ToolComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ToolComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        match update {
            AgentUpdate::ToolProgress { tool_id, chunks } => {
                let Some(pos) = self.state.active.iter().position(|a| a.tool_id == *tool_id) else {
                    return false;
                };
                self.state.active[pos].live_output.push_chunks(chunks);
                true
            }
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
            } => {
                let Some(pos) = self.state.active.iter().position(|a| a.tool_id == *tool_id) else {
                    return false;
                };
                let active = &mut self.state.active[pos];
                if let Some(m) = model {
                    active.output.subagent_model = Some(m.clone());
                }
                if let Some(t) = token_usage {
                    active.output.subagent_tokens = Some(t.clone());
                }
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
        25
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[allow(dead_code)]
fn _type_markers(_: Option<(ToolOutputChunk, TokenUsageInfo)>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        InputMode, PendingQueue,
        protocol::{ToolOutputChunk, ToolOutputStream},
        state::{LogCoordinator, StreamEvent},
        theme::{Theme, ThemeName},
        widgets::tool_widget::ToolWidget,
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

    fn seed_active(comp: &mut ToolComponent) {
        let theme = Theme::from(ThemeName::Ink);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let output = ToolWidget::new(&theme, &msgs)
            .with_tool("read_file")
            .with_arg_summary("main.rs")
            .build();
        comp.begin_active(
            0,
            "tool_1".into(),
            output,
            crate::protocol::ToolOutputBuffer::new(1024),
        );
    }

    #[test]
    fn tool_progress_feeds_live_output() {
        let mut comp = ToolComponent::new();
        seed_active(&mut comp);
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let dirty = comp.on_update(
            &AgentUpdate::ToolProgress {
                tool_id: "tool_1".into(),
                chunks: vec![ToolOutputChunk {
                    stream: ToolOutputStream::Stdout,
                    text: "compiling...".into(),
                }],
            },
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(dirty);
        assert!(
            !comp.state().active[0]
                .live_output
                .preview_lines(10)
                .is_empty()
        );
    }

    #[test]
    fn tool_meta_updates_subagent_metadata() {
        let mut comp = ToolComponent::new();
        seed_active(&mut comp);
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        comp.on_update(
            &AgentUpdate::ToolMeta {
                tool_id: "tool_1".into(),
                model: Some("subagent-model".into()),
                token_usage: Some(crate::protocol::TokenUsageInfo {
                    prompt: 1,
                    completion: 2,
                    total: 3,
                    prompt_cache_hit_tokens: 0,
                    prompt_cache_miss_tokens: 0,
                    reasoning_tokens: 0,
                }),
            },
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert_eq!(
            comp.state().active[0].output.subagent_model.as_deref(),
            Some("subagent-model")
        );
        assert_eq!(
            comp.state().active[0]
                .output
                .subagent_tokens
                .as_ref()
                .map(|t| t.total),
            Some(3)
        );
    }

    #[test]
    fn unrelated_updates_are_ignored() {
        let mut comp = ToolComponent::new();
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
