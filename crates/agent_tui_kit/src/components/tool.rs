//! Tool-activity component: full tool-card lifecycle + live output.
//!
//! Owns a [`ToolState`] and drives the whole `Step*` lifecycle: on
//! `StepStarted` it builds the running card and pushes a [`ToolEvent::Started`]
//! (the shell allocates the log placeholder rows and writes `phys_idx` back);
//! on `ToolProgress` it rebuilds the card with the live output and pushes
//! [`ToolEvent::Resize`]; on `StepFinished` / `StepFailed` /
//! `BackgroundTaskFinished` it finalizes the card (active → blocks) and
//! pushes [`ToolEvent::Finalized`]. The shell only applies log side effects
//! (placeholder rows, gap rows, scroll) — mirroring `StreamComponent`'s event
//! outbox pattern.

use std::collections::HashMap;

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{
    Component, Ctx,
    i18n::Messages,
    protocol::{AgentUpdate, PlanStep, StepResult, TokenUsageInfo, ToolOutputChunk},
    state::{ActiveToolBlock, ToolBlock, ToolState},
    theme::Theme,
    widgets::tool_widget::{ToolPhase, ToolRenderOutput, ToolWidget},
};

/// Tool-lifecycle events the shell applies after dispatch (log side effects).
///
/// The component owns the state machine; the shell owns the shared log's
/// placeholder rows, so every row-affecting transition is an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEvent {
    /// A tool card opened. The component already pushed the `ActiveToolBlock`
    /// (with `phys_idx: 0`); the shell allocates `rows` placeholder rows,
    /// ensures the pre-tool gap, and writes the real `phys_idx` back via
    /// [`ToolComponent::set_phys_idx`].
    Started { tool_id: String, rows: usize },
    /// The active card's rendered output changed height (live output growth).
    /// The shell resizes the placeholder rows at `phys_idx` and re-pins the
    /// scroll if the log was bottom-pinned.
    Resize {
        tool_id: String,
        phys_idx: usize,
        old_rows: usize,
        new_rows: usize,
    },
    /// A tool card was finalized (active → blocks). The component kept the
    /// `phys_idx` on the new `ToolBlock`; the shell resizes the placeholder
    /// rows from `old_rows` to `new_rows`. `had_active` tells the shell
    /// whether an active card existed: when `false` (a finalize arrived with
    /// no matching active card — e.g. after a restart) the shell must
    /// *allocate* the placeholder rows, not resize.
    Finalized {
        tool_id: String,
        old_rows: usize,
        new_rows: usize,
        had_active: bool,
    },
    /// A tool card was cancelled (same `tool_id` restarting without a finish).
    /// The shell removes the placeholder rows at `phys_idx`.
    Cancelled {
        tool_id: String,
        phys_idx: usize,
        rows: usize,
    },
    /// The host should surface a system message (a finalize arrived with no
    /// matching active card — e.g. background task after a restart). The text
    /// is already formatted by the component (it owns `Messages`).
    Missing { message: String },
}

pub struct ToolComponent {
    state: ToolState,
    theme: Theme,
    messages: Messages,
    /// `tool_id` → plan step index, recorded from `StepAdded` in arrival
    /// order. Mirrors the shell's `resolve_step_idx` (first plan position for
    /// a `tool_id`) so card step numbers match the plan position even when
    /// the agent's `idx` differs.
    ///
    /// **First occurrence wins** (`entry().or_insert`, never overwritten):
    /// the shell resolves a restarted `tool_id` to its *first* plan position,
    /// and the card keeps that number for its whole lifetime. The map is
    /// session-bounded (one entry per distinct `tool_id` — the same growth as
    /// the plan itself) and must **not** be pruned on finalize: pruning would
    /// make a later restart re-record the *latest* arrival index and diverge
    /// from the shell's first-position resolution.
    step_indices: HashMap<String, usize>,
    /// Monotonic counter for `step_indices` (StepAdded arrival order).
    step_count: usize,
}

impl ToolComponent {
    pub fn new(theme: Theme, messages: Messages) -> Self {
        Self {
            state: ToolState::default(),
            theme,
            messages,
            step_indices: HashMap::new(),
            step_count: 0,
        }
    }

    pub fn state(&self) -> &ToolState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ToolState {
        &mut self.state
    }

    /// Resolve a step index the way the shell did: prefer the plan position
    /// recorded from `StepAdded` for this `tool_id`, fall back to `idx`.
    fn resolve_step_idx(&self, tool_id: &str, idx: usize) -> usize {
        self.step_indices.get(tool_id).copied().unwrap_or(idx)
    }

    /// Backfill the `phys_idx` the shell allocated for a `Started` event.
    pub fn set_phys_idx(&mut self, tool_id: &str, phys_idx: usize) {
        if let Some(active) = self.state.active.iter_mut().find(|a| a.tool_id == tool_id) {
            active.phys_idx = phys_idx;
        }
    }

    /// Backfill the `phys_idx` for a `Finalized` card that was pushed without
    /// an active block (the shell's `push` fallback branch).
    pub fn set_blocks_phys_idx(&mut self, tool_id: &str, phys_idx: usize) {
        if let Some(block) = self
            .state
            .blocks
            .iter_mut()
            .rev()
            .find(|b| b.tool_id == tool_id)
        {
            block.phys_idx = phys_idx;
        }
    }

    /// Record a `StepAdded` mapping (arrival order = plan order).
    ///
    /// First occurrence wins: the shell resolves a `tool_id` to its first
    /// plan position, so a same-`tool_id` restart (StepAdded again) must
    /// keep the original index — overwriting would drift from the shell.
    fn on_step_added(&mut self, step: &PlanStep) {
        let idx = self.step_count;
        self.step_count += 1;
        self.step_indices.entry(step.tool_id.clone()).or_insert(idx);
    }

    #[allow(clippy::too_many_arguments)] // mirrors the protocol update payload
    fn on_step_started(
        &mut self,
        idx: usize,
        tool_id: &str,
        tool_name: &str,
        arg_summary: &str,
        arg_full: &str,
        presentation: &crate::protocol::ToolPresentationInfo,
        events: &mut Vec<ToolEvent>,
    ) {
        // Same tool_id restarting without a finish: drop the stale card first
        // (the shell removes its placeholder rows via Cancelled).
        if let Some(pos) = self.state.active.iter().position(|a| a.tool_id == tool_id) {
            let active = self.state.active.remove(pos);
            events.push(ToolEvent::Cancelled {
                tool_id: tool_id.to_string(),
                phys_idx: active.phys_idx,
                rows: active.output.visual_rows(false),
            });
        }
        // Full live output for subagents (based on presentation metadata).
        let is_subagent = matches!(
            &presentation.popup,
            crate::protocol::ToolPopupKind::SubagentTranscript
        );
        let step_idx = self.resolve_step_idx(tool_id, idx);
        let output = ToolWidget::new(&self.theme, &self.messages)
            .with_tool(tool_name.to_string())
            .with_arg_summary(arg_summary.to_string())
            .with_arg_full(arg_full.to_string())
            .with_step_index(step_idx)
            .with_phase(ToolPhase::Running)
            .with_duration_us(0)
            .build();
        let rows = output.visual_rows(false);
        self.state.active.push(ActiveToolBlock {
            phys_idx: 0, // shell backfills via set_phys_idx
            tool_id: tool_id.to_string(),
            output,
            live_output: if is_subagent {
                crate::protocol::ToolOutputBuffer::new_full(50_000)
            } else {
                crate::protocol::ToolOutputBuffer::new(50_000)
            },
            started_at: std::time::Instant::now(),
            subagent_child_id: None,
        });
        events.push(ToolEvent::Started {
            tool_id: tool_id.to_string(),
            rows,
        });
    }

    fn on_tool_progress(&mut self, tool_id: &str, events: &mut Vec<ToolEvent>) {
        let Some(pos) = self.state.active.iter().position(|a| a.tool_id == tool_id) else {
            return;
        };
        if self.state.active[pos].live_output.logical_line_count() == 0 {
            return;
        }
        let step_idx = self.resolve_step_idx(tool_id, 0);
        let (phys_idx, old_rows, output) = {
            let active = &self.state.active[pos];
            // Preserve subagent metadata when rebuilding the output.
            let output = ToolWidget::new(&self.theme, &self.messages)
                .with_tool(active.output.tool_name.clone())
                .with_arg_summary(active.output.arg_summary.clone())
                .with_arg_full(active.output.arg_full.clone())
                .with_step_index(step_idx)
                .with_phase(ToolPhase::Running)
                .with_duration_us(0)
                .with_live_output(&active.live_output)
                .with_subagent_model(active.output.subagent_model.clone())
                .with_subagent_tokens(active.output.subagent_tokens.clone())
                .build();
            (active.phys_idx, active.output.visual_rows(false), output)
        };
        let new_rows = output.visual_rows(false);
        self.state.active[pos].output = output;
        // Always emit: the shell refreshes scroll even when rows are
        // unchanged (live text updates while bottom-pinned).
        events.push(ToolEvent::Resize {
            tool_id: tool_id.to_string(),
            phys_idx,
            old_rows,
            new_rows,
        });
    }

    fn on_step_finished(
        &mut self,
        idx: usize,
        tool_id: &str,
        result: &StepResult,
        events: &mut Vec<ToolEvent>,
    ) {
        // Keep-live tools (e.g. `background_run`) return immediately but their
        // card keeps streaming: skip finalization here; a later
        // `BackgroundTaskFinished` closes the card with the real outcome.
        if result.presentation.keep_live {
            // `spawn_subagent` with `run_in_background` returns
            // `async_launched { <child_id> }`; parse it so the live card can
            // offer a cancel button while the child runs.
            if result.tool == "spawn_subagent"
                && let Some(child_id) = parse_async_launched(&result.message)
                && let Some(active) = self.state.active.iter_mut().find(|a| a.tool_id == tool_id)
            {
                active.subagent_child_id = Some(child_id);
            }
            return;
        }
        let is_subagent = matches!(
            result.presentation.popup,
            crate::protocol::ToolPopupKind::SubagentTranscript
        );
        let step_idx = self.resolve_step_idx(tool_id, idx);
        let mut output = ToolWidget::from_step_result(result, &self.theme, &self.messages)
            .with_step_index(step_idx)
            .build();

        // Subagent: live output holds the full conversation; detail_full would
        // otherwise only keep the final summary. Take it before the active
        // block is removed so the popup always shows the complete
        // conversation. Also carry over subagent metadata (model, tokens).
        if is_subagent
            && let Some(active) = self.state.active.iter_mut().find(|a| a.tool_id == tool_id)
        {
            let full_text = active.live_output.take_full_detail();
            if !full_text.is_empty() {
                output.detail_total_lines = full_text.lines().count();
                output.detail_full = Some(full_text);
            }
            output.subagent_model = active.output.subagent_model.take();
            output.subagent_tokens = active.output.subagent_tokens.take();
        }

        self.finalize_push(tool_id, output, events);
    }

    fn on_step_failed(
        &mut self,
        idx: usize,
        tool_id: &str,
        arg_summary: &str,
        error: &str,
        events: &mut Vec<ToolEvent>,
    ) {
        let Some(pos) = self.state.active.iter().position(|a| a.tool_id == tool_id) else {
            let msgs = &self.messages;
            // Resolve like the card header above: the system message must
            // agree with the plan position, not the raw agent index.
            let step_idx = self.resolve_step_idx(tool_id, idx);
            events.push(ToolEvent::Missing {
                message: msgs
                    .step_failed_tmpl
                    .replacen("{}", &(step_idx + 1).to_string(), 1)
                    .replacen("{}", error, 1),
            });
            return;
        };
        let active = &self.state.active[pos];
        let elapsed_us = active.started_at.elapsed().as_micros() as u64;
        let tool_name = active.output.tool_name.clone();
        // Prefer the summary carried by the failure (e.g. a web-search query
        // that was only populated at `done`), falling back to the
        // `StepStarted` value so regular tool failures keep their title.
        let arg_summary = if arg_summary.is_empty() {
            active.output.arg_summary.clone()
        } else {
            arg_summary.to_string()
        };
        let step_idx = self.resolve_step_idx(tool_id, idx);
        let output = ToolWidget::new(&self.theme, &self.messages)
            .with_tool(tool_name)
            .with_arg_summary(arg_summary)
            .with_step_index(step_idx)
            .with_phase(ToolPhase::Failed)
            .with_duration_us(elapsed_us)
            .with_detail(error.to_string())
            .build();
        self.finalize_push(tool_id, output, events);
    }

    fn on_background_task_finished(
        &mut self,
        tool_id: &str,
        success: bool,
        message: &str,
        output_text: &str,
        events: &mut Vec<ToolEvent>,
    ) {
        let Some(pos) = self.state.active.iter().position(|a| a.tool_id == tool_id) else {
            // The live card is gone (e.g. a fresh process after restart);
            // surface the outcome as a system message instead.
            let prefix = if success { "✓" } else { "✗" };
            events.push(ToolEvent::Missing {
                message: format!("{prefix} {message}"),
            });
            return;
        };
        let active = &self.state.active[pos];
        let elapsed_us = active.started_at.elapsed().as_micros() as u64;
        let tool_name = active.output.tool_name.clone();
        let arg_summary = active.output.arg_summary.clone();
        let arg_full = active.output.arg_full.clone();
        let step_idx = self.resolve_step_idx(tool_id, 0);
        let mut widget = ToolWidget::new(&self.theme, &self.messages)
            .with_tool(tool_name)
            .with_arg_summary(arg_summary)
            .with_arg_full(arg_full)
            .with_step_index(step_idx)
            .with_phase(if success {
                ToolPhase::Success
            } else {
                ToolPhase::Failed
            })
            .with_duration_us(elapsed_us)
            .with_detail(output_text.to_string());
        if !success {
            widget = widget.with_message(message.to_string());
        }
        self.finalize_push(tool_id, widget.build(), events);
    }

    /// Finalize a subagent card after a `run_in_background` child finishes.
    ///
    /// Like [`Self::on_background_task_finished`] but must carry over the full
    /// subagent transcript: keep-live finalization skips `on_step_finished`
    /// (the only place the popup transcript is populated today), so this
    /// handler re-does that carry-over before finalizing.
    fn on_subagent_finished(
        &mut self,
        tool_id: &str,
        child_id: &str,
        success: bool,
        summary: &str,
        events: &mut Vec<ToolEvent>,
    ) {
        let Some(pos) = self.state.active.iter().position(|a| a.tool_id == tool_id) else {
            // The live card is gone (e.g. a fresh process after restart);
            // surface the outcome as a system message instead.
            let prefix = if success { "✓" } else { "✗" };
            events.push(ToolEvent::Missing {
                message: format!("{prefix} Subagent {child_id} finished: {summary}"),
            });
            return;
        };
        let (tool_name, arg_summary, arg_full, elapsed_us) = {
            let active = &self.state.active[pos];
            (
                active.output.tool_name.clone(),
                active.output.arg_summary.clone(),
                active.output.arg_full.clone(),
                active.started_at.elapsed().as_micros() as u64,
            )
        };
        let step_idx = self.resolve_step_idx(tool_id, 0);
        let mut output = ToolWidget::new(&self.theme, &self.messages)
            .with_tool(tool_name)
            .with_arg_summary(arg_summary)
            .with_arg_full(arg_full)
            .with_step_index(step_idx)
            .with_phase(if success {
                ToolPhase::Success
            } else {
                ToolPhase::Failed
            })
            .with_duration_us(elapsed_us)
            .with_detail(summary.to_string())
            .build();
        // Carry over the full transcript (and subagent model/token metadata)
        // so the popup shows the complete conversation.
        if let Some(active) = self.state.active.iter_mut().find(|a| a.tool_id == tool_id) {
            let full_text = active.live_output.take_full_detail();
            if !full_text.is_empty() {
                output.detail_total_lines = full_text.lines().count();
                output.detail_full = Some(full_text);
            }
            output.subagent_model = active.output.subagent_model.take();
            output.subagent_tokens = active.output.subagent_tokens.take();
        }
        self.finalize_push(tool_id, output, events);
    }

    /// Shared finalize tail: move active → blocks and emit `Finalized`; when
    /// no active matched, push the completed block with `phys_idx: 0` and
    /// `had_active: false` (the shell's fallback branch allocates real rows
    /// via `set_blocks_phys_idx`).
    fn finalize_push(
        &mut self,
        tool_id: &str,
        output: ToolRenderOutput,
        events: &mut Vec<ToolEvent>,
    ) {
        let new_rows = output.visual_rows(false);
        if let Some(pos) = self.state.active.iter().position(|a| a.tool_id == tool_id) {
            let active = self.state.active.remove(pos);
            let old_rows = active.output.visual_rows(false);
            self.state.blocks.push(ToolBlock {
                phys_idx: active.phys_idx,
                tool_id: tool_id.to_string(),
                output,
            });
            events.push(ToolEvent::Finalized {
                tool_id: tool_id.to_string(),
                old_rows,
                new_rows,
                had_active: true,
            });
        } else {
            self.state.blocks.push(ToolBlock {
                phys_idx: 0, // shell fallback backfills via set_blocks_phys_idx
                tool_id: tool_id.to_string(),
                output,
            });
            events.push(ToolEvent::Finalized {
                tool_id: tool_id.to_string(),
                old_rows: 0,
                new_rows,
                had_active: false,
            });
        }
    }
}

impl Default for ToolComponent {
    fn default() -> Self {
        Self::new(
            Theme::from(crate::theme::ThemeName::Ink),
            Messages::by_language(crate::i18n::Language::English),
        )
    }
}

/// Transparent field access: hosts keep `app.<field>…` working after the field
/// type becomes the component (no mechanical churn at call sites).
impl std::ops::Deref for ToolComponent {
    type Target = ToolState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for ToolComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for ToolComponent {
    fn on_update(&mut self, update: &AgentUpdate, ctx: &mut Ctx<'_>) -> bool {
        match update {
            AgentUpdate::StepAdded(step) => {
                self.on_step_added(step);
                true
            }
            AgentUpdate::StepStarted {
                idx,
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                presentation,
            } => {
                self.on_step_started(
                    *idx,
                    tool_id,
                    tool_name,
                    arg_summary,
                    arg_full,
                    presentation,
                    ctx.tool_events,
                );
                true
            }
            AgentUpdate::StepFinished {
                idx,
                tool_id,
                result,
            } => {
                self.on_step_finished(*idx, tool_id, result, ctx.tool_events);
                true
            }
            AgentUpdate::StepFailed {
                idx,
                tool_id,
                arg_summary,
                error,
            } => {
                self.on_step_failed(*idx, tool_id, arg_summary, error, ctx.tool_events);
                true
            }
            AgentUpdate::ToolProgress { tool_id, chunks } => {
                if let Some(pos) = self.state.active.iter().position(|a| a.tool_id == *tool_id) {
                    self.state.active[pos].live_output.push_chunks(chunks);
                }
                self.on_tool_progress(tool_id, ctx.tool_events);
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
            AgentUpdate::BackgroundTaskFinished {
                tool_id,
                success,
                message,
                output,
            } => {
                self.on_background_task_finished(
                    tool_id,
                    *success,
                    message,
                    output,
                    ctx.tool_events,
                );
                true
            }
            AgentUpdate::SubagentFinished {
                tool_id,
                child_id,
                success,
                summary,
            } => {
                self.on_subagent_finished(tool_id, child_id, *success, summary, ctx.tool_events);
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

/// Extracts the child id from `spawn_subagent`'s `async_launched { <id> }`
/// result message. Returns `None` for anything else (sync results, errors).
fn parse_async_launched(message: &str) -> Option<String> {
    let start = message.find("async_launched {")?;
    let rest = &message[start + "async_launched {".len()..];
    let end = rest.find('}')?;
    let id = rest[..end].trim();
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        InputMode, PendingQueue,
        protocol::{
            PlanStep, StepResult, StepStatus, ToolOutputChunk, ToolOutputStream,
            ToolPresentationInfo,
        },
        state::{LogCoordinator, StreamEvent},
        theme::ThemeName,
    };

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        events: &'a mut Vec<StreamEvent>,
        tool_events: &'a mut Vec<ToolEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events: events,
            tool_events,
        }
    }

    fn comp() -> ToolComponent {
        ToolComponent::new(
            Theme::from(ThemeName::Ink),
            Messages::by_language(crate::i18n::Language::English),
        )
    }

    fn step_added(c: &mut ToolComponent, tool_id: &str) {
        c.on_update(
            &AgentUpdate::StepAdded(PlanStep::new(
                "read",
                "read_file",
                tool_id,
                std::collections::HashMap::<String, String>::new(),
            )),
            &mut ctx(
                &mut LogCoordinator::default(),
                &mut PendingQueue::default(),
                &mut Vec::new(),
                &mut Vec::new(),
            ),
        );
    }

    fn step_started(c: &mut ToolComponent, tool_id: &str) -> Vec<ToolEvent> {
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::StepStarted {
                idx: 0,
                tool_id: tool_id.into(),
                tool_name: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: "main.rs".into(),
                presentation: ToolPresentationInfo::generic("read_file"),
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        tool_events
    }

    fn tool_progress(c: &mut ToolComponent, tool_id: &str, text: &str) -> Vec<ToolEvent> {
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::ToolProgress {
                tool_id: tool_id.into(),
                chunks: vec![ToolOutputChunk {
                    stream: ToolOutputStream::Stdout,
                    text: text.into(),
                }],
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        tool_events
    }

    #[test]
    fn step_started_pushes_active_and_started_event() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let events = step_started(&mut c, "t1");
        assert_eq!(c.state().active.len(), 1);
        assert_eq!(c.state().active[0].tool_id, "t1");
        assert_eq!(c.state().active[0].phys_idx, 0, "shell backfills phys_idx");
        assert_eq!(
            events,
            vec![ToolEvent::Started {
                tool_id: "t1".into(),
                rows: c.state().active[0].output.visual_rows(false),
            }]
        );
    }

    #[test]
    fn set_phys_idx_backfills_active() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        c.set_phys_idx("t1", 42);
        assert_eq!(c.state().active[0].phys_idx, 42);
    }

    #[test]
    fn restart_same_tool_id_emits_cancel_then_started() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        c.set_phys_idx("t1", 10);
        let events = step_started(&mut c, "t1");
        assert_eq!(c.state().active.len(), 1, "stale active replaced");
        assert_eq!(events.len(), 2);
        assert!(
            matches!(events[0], ToolEvent::Cancelled { ref tool_id, phys_idx: 10, .. } if tool_id == "t1")
        );
        assert!(matches!(events[1], ToolEvent::Started { ref tool_id, .. } if tool_id == "t1"));
    }

    #[test]
    fn tool_progress_rebuilds_output_and_resizes() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        let events = tool_progress(&mut c, "t1", "live line\n");
        assert!(!c.state().active[0].live_output.preview_lines(10).is_empty());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ToolEvent::Resize { .. }));
    }

    #[test]
    fn step_finished_finalizes_and_emits_finalized() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        c.set_phys_idx("t1", 7);
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        let result = StepResult {
            tool: "read_file".into(),
            arg_summary: "main.rs".into(),
            arg_full: None,
            status: StepStatus::Success,
            message: "done".into(),
            detail: None,
            duration_us: Some(1),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("read_file"),
        };
        c.on_update(
            &AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "t1".into(),
                result,
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert!(c.state().active.is_empty());
        assert_eq!(c.state().blocks.len(), 1);
        assert_eq!(c.state().blocks[0].phys_idx, 7, "phys_idx preserved");
        assert!(matches!(tool_events[0], ToolEvent::Finalized { .. }));
    }

    #[test]
    fn keep_live_step_finished_keeps_active() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        let mut pres = ToolPresentationInfo::generic("background_run");
        pres.keep_live = true;
        let result = StepResult {
            tool: "background_run".into(),
            arg_summary: "bg".into(),
            arg_full: None,
            status: StepStatus::Success,
            message: "started".into(),
            detail: None,
            duration_us: Some(1),
            permission_label: None,
            presentation: pres,
        };
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "t1".into(),
                result,
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert_eq!(c.state().active.len(), 1, "keep-live card stays active");
        assert!(tool_events.is_empty(), "no finalize event for keep-live");
    }

    #[test]
    fn keep_live_subagent_records_child_id_for_cancel_button() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        let mut pres = ToolPresentationInfo::generic("spawn_subagent");
        pres.keep_live = true;
        let result = StepResult {
            tool: "spawn_subagent".into(),
            arg_summary: "run in background".into(),
            arg_full: None,
            status: StepStatus::Success,
            message: "async_launched { child-123 }".into(),
            detail: None,
            duration_us: Some(1),
            permission_label: None,
            presentation: pres,
        };
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "t1".into(),
                result,
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert_eq!(c.state().active.len(), 1, "keep-live card stays active");
        assert_eq!(
            c.state().active[0].subagent_child_id.as_deref(),
            Some("child-123"),
            "async subagent card must record the child id for the cancel button"
        );
    }

    #[test]
    fn parse_async_launched_extracts_id() {
        assert_eq!(
            parse_async_launched("async_launched { 7f9c-abc }"),
            Some("7f9c-abc".to_string())
        );
        assert_eq!(
            parse_async_launched("async_launched {abc}"),
            Some("abc".to_string())
        );
        assert_eq!(parse_async_launched("done"), None);
        assert_eq!(parse_async_launched("async_launched {}"), None);
        assert_eq!(parse_async_launched("async_launched {"), None);
    }

    #[test]
    fn background_task_finished_finalizes_missing_as_message() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        c.set_phys_idx("t1", 3);
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::BackgroundTaskFinished {
                tool_id: "t1".into(),
                success: true,
                message: "bg done".into(),
                output: "out".into(),
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert!(c.state().active.is_empty());
        assert_eq!(c.state().blocks.len(), 1);
        assert!(matches!(tool_events[0], ToolEvent::Finalized { .. }));

        // Missing active → formatted system message.
        let mut c2 = comp();
        let events2 = {
            let (mut log2, mut pending2, mut ev2, mut te2) = (
                LogCoordinator::default(),
                PendingQueue::default(),
                Vec::new(),
                Vec::new(),
            );
            c2.on_update(
                &AgentUpdate::BackgroundTaskFinished {
                    tool_id: "ghost".into(),
                    success: false,
                    message: "gone".into(),
                    output: String::new(),
                },
                &mut ctx(&mut log2, &mut pending2, &mut ev2, &mut te2),
            );
            te2
        };
        assert!(
            matches!(events2[0], ToolEvent::Missing { ref message } if message.contains("✗") && message.contains("gone"))
        );
    }

    #[test]
    fn step_failed_finalizes_with_error_detail() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::StepFailed {
                idx: 0,
                tool_id: "t1".into(),
                arg_summary: String::new(),
                error: "boom".into(),
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert!(c.state().active.is_empty());
        assert_eq!(c.state().blocks.len(), 1);
        assert!(c.state().blocks[0].output.detail_full.as_deref() == Some("boom"));
        assert!(matches!(tool_events[0], ToolEvent::Finalized { .. }));
    }

    #[test]
    fn restart_keeps_first_step_mapping() {
        // Same tool_id restarts: the card must keep the FIRST plan position
        // (mirroring the shell's first-occurrence resolution), not the
        // latest arrival index.
        let mut c = comp();
        step_added(&mut c, "t1");
        step_added(&mut c, "t1"); // restart: arrival order says index 1
        let _ = step_started(&mut c, "t1");
        assert!(
            c.state().active[0].output.title_raw.starts_with("1. "),
            "card keeps first step number, got: {}",
            c.state().active[0].output.title_raw
        );
    }

    #[test]
    fn step_failed_without_active_emits_missing() {
        let mut c = comp();
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::StepFailed {
                idx: 2,
                tool_id: "ghost".into(),
                arg_summary: String::new(),
                error: "nope".into(),
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert!(
            matches!(tool_events[0], ToolEvent::Missing { ref message } if message.contains("3") && message.contains("nope"))
        );
    }

    #[test]
    fn step_failed_missing_uses_resolved_step_number() {
        // Raw agent idx (7) differs from the plan position (0): the system
        // message must use the resolved number like the card header.
        let mut c = comp();
        step_added(&mut c, "t1");
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::StepFailed {
                idx: 7,
                tool_id: "t1".into(),
                arg_summary: String::new(),
                error: "boom".into(),
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert!(
            matches!(tool_events[0], ToolEvent::Missing { ref message } if message.contains("Step 1 failed") && message.contains("boom")),
            "expected resolved step 1, got: {:?}",
            tool_events[0]
        );
    }

    #[test]
    fn finalize_without_active_marks_had_active_false() {
        // A finalize with no matching active card (e.g. after a restart)
        // must tell the shell to allocate rows, not resize.
        let mut c = comp();
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        let result = StepResult {
            tool: "read_file".into(),
            arg_summary: "main.rs".into(),
            arg_full: None,
            status: StepStatus::Success,
            message: "done".into(),
            detail: None,
            duration_us: Some(1),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("read_file"),
        };
        c.on_update(
            &AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "ghost".into(),
                result,
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert_eq!(c.state().blocks.len(), 1);
        assert_eq!(c.state().blocks[0].phys_idx, 0);
        assert!(matches!(
            tool_events[0],
            ToolEvent::Finalized {
                had_active: false,
                ..
            }
        ));
    }

    #[test]
    fn unrelated_updates_are_ignored() {
        let mut c = comp();
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        let dirty = c.on_update(
            &AgentUpdate::TaskComplete("done".into()),
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert!(!dirty);
        assert!(tool_events.is_empty());
    }

    #[test]
    fn tool_meta_updates_subagent_metadata() {
        let mut c = comp();
        step_added(&mut c, "t1");
        let _ = step_started(&mut c, "t1");
        let (mut log, mut pending, mut events, mut tool_events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
            Vec::new(),
        );
        c.on_update(
            &AgentUpdate::ToolMeta {
                tool_id: "t1".into(),
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
            &mut ctx(&mut log, &mut pending, &mut events, &mut tool_events),
        );
        assert_eq!(
            c.state().active[0].output.subagent_model.as_deref(),
            Some("subagent-model")
        );
    }
}
