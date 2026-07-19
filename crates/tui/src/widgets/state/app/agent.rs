use crate::render::render_md::{format_table, is_horizontal_rule, render_markdown_tui};
use crate::widgets::state::*;
use crate::widgets::tool_widget::{ToolPhase, ToolWidget};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ScrollbarState;
use std::time::Instant;
use tact_protocol::{
    AccountError, AccountUpdate, AgentErrorKind, AgentUpdate, PlanStep, StepResult, ThinkingChunk,
    ToolOutputBuffer, ToolOutputChunk,
};

const CODE_BG: Color = Color::Rgb(30, 35, 50);
const CODE_FG: Color = Color::Rgb(200, 200, 210);
const STREAMING_INDICATOR: &str = " ▌";

fn resolve_step_idx(steps: &[PlanStep], tool_id: &str, idx: usize) -> usize {
    if !tool_id.is_empty()
        && let Some(found) = steps.iter().position(|s| s.tool_id == tool_id)
    {
        return found;
    }
    idx
}

fn elapsed_secs_since(start: chrono::DateTime<chrono::Local>) -> i64 {
    chrono::Local::now()
        .signed_duration_since(start)
        .num_seconds()
        .max(0)
}

impl App {
    fn freeze_last_prompt_cost(&mut self) {
        if let Some(start) = self.task_start_time.take() {
            self.last_prompt_elapsed_secs = Some(elapsed_secs_since(start));
        }
    }

    pub(crate) fn handle_agent_update(&mut self, update: AgentUpdate) {
        self.dirty = true;
        // Safety net: close an open thinking region on content-producing updates
        // that are not ThinkingChunk. Explicit ThinkingChunk::Finished is preferred;
        // TokenUsage / ModelInfo must not close the region (they can arrive mid-stream).
        match &update {
            AgentUpdate::ThinkingChunk(_)
            | AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolProgress { .. } => {}
            _ => {
                self.flush_and_close_thinking();
            }
        }
        // Remove the loading placeholder on any content-producing update.
        // Metadata-only updates (TokenUsage, Balance, UsageQuota, ModelInfo)
        // should NOT remove the placeholder since they don't produce visible content.
        match &update {
            AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolProgress { .. } => {
                // Metadata only, no content: keep the loading placeholder.
            }
            _ => {
                self.remove_loading_placeholder();
            }
        }
        match update {
            AgentUpdate::StepAdded(step) => self.on_step_added(step),
            AgentUpdate::StepStarted {
                idx,
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
            } => self.on_step_started(idx, tool_id, tool_name, arg_summary, arg_full),
            AgentUpdate::StepFinished {
                idx,
                tool_id,
                result,
            } => self.on_step_finished(idx, tool_id, result),
            AgentUpdate::StepFailed {
                idx,
                tool_id,
                error,
            } => self.on_step_failed(idx, tool_id, error),
            AgentUpdate::TaskComplete(summary) => {
                // Task complete: flush leftover streaming lines
                self.flush_stream_pending();
                // Don't re-render summary into messages (StreamChunk already displayed it).
                // Summary is only saved to task_history for history viewing.
                if let Some(entry) = self.task_history.last_mut() {
                    entry.summary = summary;
                }
                // Trailing separator: bumps messages.len() to rebuild the visual wrap
                // cache and marks the end of this response.
                self.add_task_end_separator();
                if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
                    self.log_scroll.offset = u16::MAX;
                }
                self.status = Status::Done;
                self.freeze_last_prompt_cost();
                self.task_done_time = Some(chrono::Local::now());
                // TODO Add task stats block
            }
            AgentUpdate::TaskCancelled => {
                // Cancel exits without TaskComplete; must leave Planning/Executing
                // or Enter keeps flashing input_busy_msg.
                self.flush_stream_pending();
                self.add_task_end_separator();
                if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
                    self.log_scroll.offset = u16::MAX;
                }
                self.status = Status::Idle;
                self.freeze_last_prompt_cost();
                self.task_done_time = None;
            }
            // Error handling
            AgentUpdate::Error(AgentErrorKind::Other(msg)) => {
                // Fatal error: flush leftover streaming lines
                self.flush_stream_pending();
                let msgs = self.msgs();
                self.add_system_message(msgs.error_tmpl.replace("{}", &msg));
                self.status = Status::Idle;
                self.freeze_last_prompt_cost();
            }
            // Update token usage info
            AgentUpdate::TokenUsage(usage) => {
                self.status_bar.token_prompt = usage.prompt;
                self.status_bar.token_completion = usage.completion;
                self.status_bar.token_total = usage.total;
                self.status_bar.token_cache_hit = usage.prompt_cache_hit_tokens;
                self.status_bar.token_cache_miss = usage.prompt_cache_miss_tokens;
                self.status_bar.token_reasoning = usage.reasoning_tokens;
            }
            // Update model info
            AgentUpdate::ModelInfo(params) => {
                self.status_bar.model_name = params.model;
                self.status_bar.model_max_tokens = params.max_tokens;
                self.status_bar.model_thinking_budget = params.thinking_budget;
                self.status_bar.model_reasoning_effort = params.reasoning_effort;
            }
            // Add system message
            AgentUpdate::Info(msg) => {
                self.add_system_message(msg);
            }
            AgentUpdate::RequestSelect {
                prompt,
                options,
                respond,
                log_confirm,
            } => {
                self.select_kind = SelectKind::Agent;
                self.select.set(prompt, options, respond, log_confirm);
                self.input_mode = InputMode::Select;
            }
            AgentUpdate::RequestMultiSelect {
                prompt,
                options,
                respond,
            } => {
                self.select_kind = SelectKind::Agent;
                // Choice is shown on the ask_user tool meta row; no duplicate log line.
                self.select.set_multi(prompt, options, respond, false);
                self.input_mode = InputMode::Select;
            }
            AgentUpdate::ThinkingChunk(chunk) => {
                match chunk {
                    ThinkingChunk::Started => {
                        self.begin_thinking_block();
                    }
                    ThinkingChunk::Delta(text) => {
                        // Started may be missing on older producers — open on first delta.
                        if self.thinking.active.is_none() {
                            self.begin_thinking_block();
                        }
                        self.append_thinking_delta(&text);
                    }
                    ThinkingChunk::Finished => {
                        self.flush_and_close_thinking();
                    }
                }
            }
            AgentUpdate::ToolProgress { tool_id, chunks } => {
                self.on_tool_progress(&tool_id, &chunks)
            }
            AgentUpdate::StreamChunk(text) => self.apply_stream_chunk(text),
        }
        // Unified tail scroll state refresh, covering cases where helpers like
        // flush_and_close_thinking / flush_stream_pending inserted messages without
        // updating scroll (most arms call add_system_message independently,
        // StreamChunk / ThinkingChunk also update separately; this redundant call is
        // cheap and harmless).
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
    }

    fn on_step_added(&mut self, step: PlanStep) {
        // Flush leftover streaming text, preventing LLM output from appearing
        // between StepAdded and StepStarted.
        self.flush_stream_pending();
        let idx = self.plan.steps.len();
        self.plan.steps.push(step.clone());
        self.plan
            .steps_set
            .insert(step.tool_id.clone(), step.clone());
        self.plan.collapsed.push(false);
        // Don't change current_step or total — the step hasn't started yet.
        // Ensure there is an Executing status before StepStarted arrives.
        self.ensure_executing_status(idx);
        self.plan.scroll_state = ScrollbarState::new(self.plan.steps.len().saturating_sub(1));
    }

    fn on_step_started(
        &mut self,
        idx: usize,
        tool_id: String,
        tool_name: String,
        arg_summary: String,
        arg_full: String,
    ) {
        let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
        self.flush_stream_pending();
        // Same tool_id restarting without a finish: drop stale placeholder rows.
        self.cancel_active_tool(&tool_id);
        if let Status::Executing {
            current_step,
            total,
        } = &mut self.status
        {
            *current_step = idx;
            if idx >= *total {
                *total = idx + 1;
            }
        }
        let msgs = self.msgs();
        let output = ToolWidget::new(&self.theme, &msgs)
            .with_tool(tool_name)
            .with_arg_summary(arg_summary)
            .with_arg_full(arg_full)
            .with_step_index(idx)
            .with_phase(ToolPhase::Running)
            .with_duration_us(0)
            .build();
        let phys_idx = self.push_tool_placeholder_rows(&output);
        self.tools.active.push(ActiveToolBlock {
            phys_idx,
            tool_id,
            output,
            live_output: ToolOutputBuffer::new(50_000),
            started_at: Instant::now(),
        });
        self.refresh_tool_log_scroll();
    }

    fn on_tool_progress(&mut self, tool_id: &str, chunks: &[ToolOutputChunk]) {
        let Some(pos) = self
            .tools
            .active
            .iter()
            .position(|active| active.tool_id == tool_id)
        else {
            return;
        };
        let was_pinned = self.log_scroll.offset == u16::MAX;
        self.tools.active[pos].live_output.push_chunks(chunks);
        if self.tools.active[pos].live_output.logical_line_count() == 0 {
            return;
        }

        let active = &self.tools.active[pos];
        let phys_idx = active.phys_idx;
        let old_rows = active.output.visual_rows(false);
        let tool_name = active.output.tool_name.clone();
        let arg_summary = active.output.arg_summary.clone();
        let arg_full = active.output.arg_full.clone();
        let live_output = active.live_output.clone();
        let step_idx = resolve_step_idx(&self.plan.steps, tool_id, 0);
        let msgs = self.msgs();
        let output = ToolWidget::new(&self.theme, &msgs)
            .with_tool(tool_name)
            .with_arg_summary(arg_summary)
            .with_arg_full(arg_full)
            .with_step_index(step_idx)
            .with_phase(ToolPhase::Running)
            .with_duration_us(0)
            .with_live_output(&live_output)
            .build();
        let new_rows = output.visual_rows(false);
        self.resize_tool_placeholder_rows(phys_idx, old_rows, new_rows);
        self.tools.active[pos].output = output;
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        if was_pinned {
            self.log_scroll.offset = u16::MAX;
        }
    }

    fn on_step_finished(&mut self, idx: usize, tool_id: String, result: StepResult) {
        let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
        self.flush_stream_pending();
        let msgs = self.msgs();
        let output = ToolWidget::from_step_result(&result, &self.theme, &msgs)
            .with_step_index(idx)
            .build();
        self.finalize_tool_block(&tool_id, output);

        if let Some(step) = self.plan.steps.get_mut(idx) {
            step.output = Some(result.message);
        }
    }

    fn on_step_failed(&mut self, idx: usize, tool_id: String, error: String) {
        let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
        self.flush_stream_pending();
        if let Some(active) = self.tools.active.iter().find(|a| a.tool_id == tool_id) {
            let elapsed_us = active.started_at.elapsed().as_micros() as u64;
            let tool_name = active.output.tool_name.clone();
            let arg_summary = active.output.arg_summary.clone();
            let msgs = self.msgs();
            let output = ToolWidget::new(&self.theme, &msgs)
                .with_tool(tool_name)
                .with_arg_summary(arg_summary)
                .with_step_index(idx)
                .with_phase(ToolPhase::Failed)
                .with_duration_us(elapsed_us)
                .with_detail(error)
                .build();
            self.finalize_tool_block(&tool_id, output);
        } else {
            let msgs = self.msgs();
            self.add_system_message(
                msgs.step_failed_tmpl
                    .replacen("{}", &(idx + 1).to_string(), 1)
                    .replacen("{}", &error, 1),
            );
        }
        self.status = Status::Idle;
        self.freeze_last_prompt_cost();
    }

    fn apply_stream_chunk(&mut self, text: String) {
        self.ensure_gap_after_tools();
        // Thinking region is closed by the safety gate above when still open.
        self.stream.buffer.push_str(&text);

        // Line-level buffering: code blocks accumulate by complete unit,
        // table rows accumulate by table, normal lines accumulate by paragraph
        let mut completed = Vec::new();
        while let Some(idx) = self.stream.buffer.find('\n') {
            let line = self.stream.buffer[..idx].to_string();
            self.stream.buffer = self.stream.buffer[idx + 1..].to_string();

            let trimmed = line.trim();
            let is_code_fence = trimmed.starts_with("```");
            let is_code_fence_close = trimmed == "```" && self.stream.code_block;

            if is_code_fence_close {
                // Completed: replace streaming placeholders with a sized blank region,
                // then store a CodeBlock overlay for card rendering.
                const MAX_CODE_PREVIEW: usize = 30;
                let lang = std::mem::take(&mut self.stream.code_block_lang);
                let lines = std::mem::take(&mut self.stream.code_block_buffer);

                if let Some(start_idx) = self.stream.code_block_start_idx.take() {
                    let stream_end = start_idx + self.stream.code_block_line_count;

                    if !lines.is_empty() {
                        let code_text = format!("```{}\n{}\n```", lang, lines.join("\n"));
                        let (styled, _) = render_markdown_tui(&code_text, &self.theme);
                        let placeholder_count = styled.len().min(MAX_CODE_PREVIEW) + 2; // +2 for card border
                        let placeholders: Vec<Line<'static>> =
                            (0..placeholder_count).map(|_| Line::from("")).collect();
                        let raw_placeholders: Vec<String> =
                            (0..placeholder_count).map(|_| String::new()).collect();
                        self.splice_msgs(
                            start_idx..stream_end,
                            placeholders,
                            raw_placeholders,
                            RawMessageType::LLM,
                        );
                        self.code_blocks.push(CodeBlock {
                            start_idx,
                            end_idx: start_idx + placeholder_count,
                            lang,
                            content: lines.join("\n"),
                            styled,
                        });
                    } else {
                        self.drain_msgs(start_idx..stream_end);
                    }
                } else if !lines.is_empty() {
                    let code_text = format!("```{}\n{}\n```", lang, lines.join("\n"));
                    let (styled, raw) = render_markdown_tui(&code_text, &self.theme);
                    completed.extend(styled.into_iter().zip(raw));
                }
                self.stream.code_block = false;
                self.stream.code_block_line_count = 0;
            } else if self.stream.code_block {
                // Streaming: update previous line (remove indicator), append new line with indicator
                self.stream.code_block_buffer.push(line.clone());

                let prev_idx = self.messages.len().saturating_sub(1);
                if self.stream.code_block_line_count > 1
                    && let Some(prev_raw) = self.raw_messages.get_mut(prev_idx)
                    && prev_raw.ends_with(STREAMING_INDICATOR)
                {
                    let clean = prev_raw.trim_end_matches(STREAMING_INDICATOR).to_string();
                    *prev_raw = clean.clone();
                    self.messages[prev_idx] = Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::DarkGray).bg(CODE_BG)),
                        Span::styled(clean, Style::default().fg(CODE_FG).bg(CODE_BG)),
                    ]);
                }

                let display_line = format!("{}{}", line, STREAMING_INDICATOR);
                self.append_msg(
                    Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::DarkGray).bg(CODE_BG)),
                        Span::styled(display_line, Style::default().fg(CODE_FG).bg(CODE_BG)),
                    ]),
                    line,
                    RawMessageType::LLM,
                );
                self.stream.code_block_line_count += 1;
            } else if is_code_fence {
                // Open new code block: flush pending content first
                if !self.stream.paragraph.is_empty() {
                    let paragraph = std::mem::take(&mut self.stream.paragraph);
                    let (styled, raw) = render_markdown_tui(&paragraph, &self.theme);
                    completed.extend(styled.into_iter().zip(raw));
                }
                if !self.stream.table_buffer.is_empty() {
                    let (styled, raw) = format_table(&self.stream.table_buffer, &self.theme);
                    completed.extend(styled.into_iter().zip(raw));
                    self.stream.table_buffer.clear();
                }

                // Flush completed lines so start_idx is accurate
                for (styled_line, raw_line) in completed.drain(..) {
                    self.append_msg(styled_line, raw_line, RawMessageType::LLM);
                }

                let lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();
                self.stream.code_block = true;
                self.stream.code_block_buffer.clear();
                self.stream.code_block_lang = lang.clone();
                self.stream.code_block_start_idx = Some(self.messages.len());
                self.stream.code_block_line_count = 1;

                // Container header: ╭─ lang ─────
                let label = if lang.is_empty() {
                    "code".to_string()
                } else {
                    lang.clone()
                };
                let header_text = format!("╭─ {} ", label);
                self.append_msg(
                    Line::from(Span::styled(
                        header_text.clone(),
                        Style::default().fg(Color::DarkGray).bg(CODE_BG),
                    )),
                    format!("```{}", lang),
                    RawMessageType::LLM,
                );
            } else {
                // Regular line handling
                let is_table_line = trimmed.starts_with('|');
                let is_blank = trimmed.is_empty();
                let is_hr = is_horizontal_rule(&line);

                if is_table_line {
                    if !self.stream.paragraph.is_empty() {
                        let paragraph = std::mem::take(&mut self.stream.paragraph);
                        let (styled, raw) = render_markdown_tui(&paragraph, &self.theme);
                        completed.extend(styled.into_iter().zip(raw));
                    }
                    self.stream.table_buffer.push(line);
                } else if is_blank || is_hr {
                    if !self.stream.paragraph.is_empty() {
                        let paragraph = std::mem::take(&mut self.stream.paragraph);
                        let (styled, raw) = render_markdown_tui(&paragraph, &self.theme);
                        completed.extend(styled.into_iter().zip(raw));
                    }
                    if !self.stream.table_buffer.is_empty() {
                        let (styled, raw) = format_table(&self.stream.table_buffer, &self.theme);
                        completed.extend(styled.into_iter().zip(raw));
                        self.stream.table_buffer.clear();
                    }
                    if is_hr {
                        // Discard horizontal rules
                    } else {
                        completed.push((Line::from(""), String::new()));
                    }
                } else {
                    if !self.stream.table_buffer.is_empty() {
                        let (styled, raw) = format_table(&self.stream.table_buffer, &self.theme);
                        completed.extend(styled.into_iter().zip(raw));
                        self.stream.table_buffer.clear();
                    }
                    if !self.stream.paragraph.is_empty() {
                        self.stream.paragraph.push('\n');
                    }
                    self.stream.paragraph.push_str(&line);
                }
            }
        }

        for (styled_line, raw_line) in completed {
            self.append_msg(styled_line, raw_line, RawMessageType::LLM);
        }

        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        // Auto-scroll to bottom (u16::MAX clipped by render_log_panel to visual line count)
        self.log_scroll.offset = u16::MAX;
    }

    /// Apply an account-service update (balance / usage quota).
    ///
    /// These updates live on a separate channel from the agent runtime so that
    /// provider-specific account state does not leak into the agent protocol.
    pub(crate) fn handle_account_update(&mut self, update: AccountUpdate) {
        self.dirty = true;
        match update {
            AccountUpdate::Balance(info) => self.account.set_balance(info),
            AccountUpdate::UsageQuota(info) => self.account.set_quota(info),
            AccountUpdate::Error(err) => {
                // Only clear on permanent unsupported; keep last-known values
                // across transient poll / network failures.
                if matches!(err, AccountError::NotSupported) {
                    self.account.clear();
                }
                self.flash_msg = Some((err.to_string(), std::time::Instant::now()));
            }
        }
    }

    /// Revert `Done` → `Idle` after 2s (shared with `run_tui` main loop).
    pub(crate) fn maybe_expire_done_status(&mut self) {
        if let Status::Done = self.status
            && let Some(done_time) = self.task_done_time
            && chrono::Local::now()
                .signed_duration_since(done_time)
                .num_seconds()
                >= 2
        {
            self.status = Status::Idle;
            self.task_done_time = None;
            self.dirty = true;
        }
    }

    /// Clear `flash_msg` after 3s (shared with `run_tui` main loop).
    pub(crate) fn maybe_clear_flash_msg(&mut self) {
        if self
            .flash_msg
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs() >= 3)
        {
            self.flash_msg = None;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use crate::widgets::state::{App, Status};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tact_protocol::{
        AccountError, AccountUpdate, AgentErrorKind, AgentUpdate, PlanStep, StepResult, StepStatus,
        ThinkingChunk, ToolOutputChunk,
    };
    use tokio::sync::mpsc::unbounded_channel;

    fn make_app() -> App {
        let (_agent_tx, agent_rx) = unbounded_channel();
        let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        App::new(
            agent_rx,
            None,
            user_cmd_tx,
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        )
    }

    fn seed_running_bash(app: &mut App, tool_id: &str) {
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "run command",
            "bash",
            tool_id,
            HashMap::from([("command".to_string(), "long-command".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: tool_id.to_string(),
            tool_name: "bash".into(),
            arg_summary: "long-command".into(),
            arg_full: "long-command".into(),
        });
    }

    #[test]
    fn progress_expands_once_to_five_output_rows() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        let initial_rows = app.tools.active[0].output.visual_rows(false);

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("one\n")],
        });
        let live_rows = app.tools.active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("two\nthree\n")],
        });

        assert!(live_rows > initial_rows);
        assert_eq!(app.tools.active[0].output.visual_rows(false), live_rows);
        assert_eq!(app.tools.active[0].output.detail_preview.len(), 5);
    }

    #[test]
    fn progress_does_not_repin_scrolled_log() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        app.log_scroll.offset = 3;

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("line\n")],
        });

        assert_eq!(app.log_scroll.offset, 3);
    }

    #[test]
    fn progress_keeps_open_thinking_and_ignores_unknown_tool_ids() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "still thinking".into(),
        )));

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "unknown".into(),
            chunks: vec![ToolOutputChunk::stdout("ignored\n")],
        });

        assert!(app.thinking.active.is_some());
        assert_eq!(app.tools.active[0].output.visual_rows(false), 2);
    }

    #[test]
    fn active_bash_popup_uses_buffered_output() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("live line\n")],
        });
        let phys_idx = app.tools.active[0].phys_idx;

        app.open_diff_popup(phys_idx);

        let content = app
            .tools
            .popup
            .as_ref()
            .and_then(|popup| popup.inline_content.as_deref())
            .unwrap_or_default();
        assert!(content.contains("live line"), "popup content: {content}");
    }

    #[test]
    fn completed_bash_collapses_live_card_and_ignores_late_progress() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("live line\n")],
        });
        let live_rows = app.tools.active[0].output.visual_rows(false);

        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "b1".into(),
            result: StepResult {
                tool: "bash".into(),
                arg_summary: "long-command".into(),
                arg_full: Some("long-command".into()),
                status: StepStatus::Success,
                message: "live line".into(),
                detail: Some("live line\n".into()),
                duration_us: Some(100),
                permission_label: None,
            },
        });
        let completed_rows = app.tools.blocks[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("late\n")],
        });

        assert!(completed_rows < live_rows);
        assert!(app.tools.active.is_empty());
        assert_eq!(
            app.tools.blocks[0].output.detail_full.as_deref(),
            Some("live line\n")
        );
    }

    #[test]
    fn maybe_expire_done_status_clears_stale_done() {
        let mut app = make_app();
        app.status = Status::Done;
        app.task_done_time = Some(chrono::Local::now() - chrono::Duration::seconds(5));
        app.maybe_expire_done_status();
        assert!(matches!(app.status, Status::Idle));
    }

    #[test]
    fn usage_quota_update_sets_usage_and_repaints() {
        use tact_protocol::{UsageQuotaInfo, UsageQuotaWindow};

        let (_tx, account_rx) = unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.dirty = false;
        app.handle_account_update(AccountUpdate::UsageQuota(UsageQuotaInfo {
            is_available: true,
            windows: vec![
                UsageQuotaWindow {
                    label: "week".into(),
                    limit: Some(100.0),
                    remaining: Some(74.0),
                    reset_time: None,
                },
                UsageQuotaWindow {
                    label: "5h".into(),
                    limit: Some(100.0),
                    remaining: Some(85.0),
                    reset_time: None,
                },
            ],
            membership_level: None,
        }));

        assert!(app.account.quota.is_some());
        assert!(app.account.balance.is_none());
        assert!(app.dirty);
        assert!(crate::should_repaint(&app));
    }

    #[test]
    fn balance_update_sets_balance_info() {
        use tact_protocol::{BalanceEntry, BalanceInfo};

        let (_tx, account_rx) = unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.handle_account_update(AccountUpdate::Balance(BalanceInfo {
            is_available: true,
            balance_infos: vec![BalanceEntry {
                currency: "CNY".into(),
                total_balance: 99.00,
                granted_balance: 99.00,
                topped_up_balance: 0.00,
            }],
        }));

        assert!(app.account.balance.is_some());
        assert!(
            app.account
                .balance
                .as_ref()
                .is_some_and(|b| b.balance_infos.iter().any(|e| e.currency == "CNY"))
        );
        assert!(app.dirty, "balance update should trigger repaint");
        assert!(
            crate::should_repaint(&app),
            "idle balance update must pass repaint gate so bottom row is drawn"
        );
    }

    #[test]
    fn balance_update_on_idle_repaints_bottom_amount_row() {
        use tact_protocol::{BalanceEntry, BalanceInfo};

        let (_tx, account_rx) = unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.dirty = false;
        app.status = Status::Idle;
        app.handle_account_update(AccountUpdate::Balance(BalanceInfo {
            is_available: true,
            balance_infos: vec![BalanceEntry {
                currency: "CNY".into(),
                total_balance: 88.50,
                granted_balance: 80.00,
                topped_up_balance: 8.50,
            }],
        }));

        assert!(crate::should_repaint(&app));

        let text = crate::render::test_harness::render_app_text(&mut app, 120, 12);
        assert!(
            text.contains("88.50") || text.contains("CNY"),
            "balance amount should append on bottom bar row 1, got:\n{text}"
        );
    }

    #[test]
    fn step_added_then_task_complete_reaches_done() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "main.rs".to_string())]),
        )));
        assert!(matches!(app.status, Status::Executing { .. }));

        app.handle_agent_update(AgentUpdate::TaskComplete("All done.".into()));
        assert!(matches!(app.status, Status::Done));
        assert!(app.task_done_time.is_some());
    }

    #[test]
    fn step_finished_updates_plan_output() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "main.rs".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "tool_read_1".into(),
            tool_name: "read_file".into(),
            arg_summary: "main.rs".into(),
            arg_full: "main.rs".into(),
        });
        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "tool_read_1".into(),
            result: StepResult {
                tool: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("file body".into()),
                duration_us: Some(1),
                permission_label: None,
            },
        });

        assert_eq!(app.plan.steps[0].output.as_deref(), Some("ok"));
    }

    #[test]
    fn step_failed_sets_idle() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "missing.txt".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepFailed {
            idx: 0,
            tool_id: "tool_read_1".into(),
            error: "file not found".into(),
        });
        assert!(matches!(app.status, Status::Idle));
    }

    #[test]
    fn error_other_sets_idle_and_adds_message() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::Error(AgentErrorKind::Other(
            "LLM unavailable".into(),
        )));
        assert!(matches!(app.status, Status::Idle));
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("LLM unavailable")),
            "error message should appear in log: {:?}",
            app.raw_messages
        );
    }

    #[test]
    fn info_update_appends_system_message() {
        let mut app = make_app();
        let before = app.raw_messages.len();
        app.handle_agent_update(AgentUpdate::Info("Cancelling...".into()));
        assert!(app.raw_messages.len() > before);
        assert!(
            app.raw_messages
                .last()
                .is_some_and(|m| m.contains("Cancelling"))
        );
    }

    #[test]
    fn task_cancelled_clears_busy_status_to_idle() {
        let mut app = make_app();
        app.status = Status::Planning;
        app.handle_agent_update(AgentUpdate::TaskCancelled);
        assert!(
            matches!(app.status, Status::Idle),
            "TaskCancelled must clear Planning/Executing so new prompts can submit"
        );
    }

    #[test]
    fn stream_chunk_then_task_complete_reaches_done() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::StreamChunk("Streaming answer.".into()));
        app.handle_agent_update(AgentUpdate::TaskComplete("Streaming answer.".into()));
        assert!(matches!(app.status, Status::Done));
    }

    #[test]
    fn token_usage_updates_status_bar() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::TokenUsage(tact_protocol::TokenUsageInfo {
            prompt: 100,
            completion: 50,
            total: 150,
            prompt_cache_hit_tokens: 10,
            prompt_cache_miss_tokens: 90,
            reasoning_tokens: 5,
        }));
        assert_eq!(app.status_bar.token_prompt, 100);
        assert_eq!(app.status_bar.token_completion, 50);
        assert_eq!(app.status_bar.token_total, 150);
        assert_eq!(app.status_bar.token_reasoning, 5);
    }

    #[test]
    fn request_select_enters_select_mode() {
        use crate::widgets::state::InputMode;

        let mut app = make_app();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.handle_agent_update(AgentUpdate::RequestSelect {
            prompt: "Allow bash?".into(),
            options: vec!["Yes".into(), "No".into()],
            respond: tx,
            log_confirm: false,
        });
        assert!(matches!(app.input_mode, InputMode::Select));
        assert!(app.select.prompt.contains("Allow bash"));
    }

    #[test]
    fn thinking_chunk_flushes_on_stream() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "reasoning line".into(),
        )));
        assert!(app.thinking.active.is_some());
        app.handle_agent_update(AgentUpdate::StreamChunk("final answer".into()));
        assert!(app.thinking.active.is_none());
    }

    #[test]
    fn model_info_updates_status_bar() {
        use tact_protocol::ModelCallParams;

        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ModelInfo(ModelCallParams {
            model: "mock-model".into(),
            max_tokens: 4096,
            thinking_budget: Some(32_000),
            reasoning_effort: Some("high".into()),
            extra_body: None,
        }));
        assert_eq!(app.status_bar.model_name, "mock-model");
        assert_eq!(app.status_bar.model_max_tokens, 4096);
        assert_eq!(app.status_bar.model_thinking_budget, Some(32_000));
        assert_eq!(
            app.status_bar.model_reasoning_effort.as_deref(),
            Some("high")
        );
    }

    #[test]
    fn multiple_step_added_grows_plan() {
        let mut app = make_app();
        for (i, path) in ["a.rs", "b.rs"].into_iter().enumerate() {
            app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
                format!("read {path}"),
                "read_file",
                format!("tool_{i}"),
                HashMap::from([("path".to_string(), path.to_string())]),
            )));
        }
        assert_eq!(app.plan.steps.len(), 2);
    }

    #[test]
    fn balance_query_failed_sets_flash_message() {
        let (_tx, account_rx) = unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.account.balance = Some(tact_protocol::BalanceInfo {
            is_available: true,
            balance_infos: vec![],
        });
        app.handle_account_update(AccountUpdate::Error(AccountError::QueryFailed(
            "network down".into(),
        )));
        assert!(
            app.flash_msg
                .as_ref()
                .is_some_and(|(msg, _)| msg.contains("network down"))
        );
        assert!(
            app.account.balance.is_some(),
            "transient query failures must keep the last successful balance"
        );
    }

    #[test]
    fn step_started_then_finished_stays_executing_until_task_complete() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read",
            "read_file",
            "t1",
            HashMap::from([("path".to_string(), "a.rs".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "t1".into(),
            tool_name: "read_file".into(),
            arg_summary: "a.rs".into(),
            arg_full: "a.rs".into(),
        });
        assert!(matches!(app.status, Status::Executing { .. }));
        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "t1".into(),
            result: StepResult {
                tool: "read_file".into(),
                arg_summary: "a.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: None,
                duration_us: Some(1),
                permission_label: None,
            },
        });
        assert!(
            !matches!(app.status, Status::Done),
            "single step finish should not mark task done"
        );
    }

    #[test]
    fn balance_not_supported_clears_balance_info() {
        let (_tx, account_rx) = unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.account.balance = Some(tact_protocol::BalanceInfo {
            is_available: true,
            balance_infos: vec![],
        });
        app.handle_account_update(AccountUpdate::Error(AccountError::NotSupported));
        assert!(app.account.balance.is_none());
    }

    #[test]
    fn thinking_chunks_accumulate_before_non_thinking_update() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "part1 ".into(),
        )));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "part2".into(),
        )));
        assert!(
            app.thinking
                .active
                .as_ref()
                .unwrap()
                .content
                .contains("part1")
        );
        assert!(
            app.thinking
                .active
                .as_ref()
                .unwrap()
                .content
                .contains("part2")
        );
        app.handle_agent_update(AgentUpdate::Info("done thinking".into()));
        assert!(app.thinking.active.is_none());
    }

    #[test]
    fn thinking_finished_closes_without_other_update() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "done thinking\n".into(),
        )));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        assert!(app.thinking.active.is_none());
        assert!(!app.thinking.blocks.is_empty());
    }

    #[test]
    fn token_usage_does_not_close_open_thinking() {
        use tact_protocol::TokenUsageInfo;

        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "still thinking".into(),
        )));
        app.handle_agent_update(AgentUpdate::TokenUsage(TokenUsageInfo {
            prompt: 1,
            completion: 2,
            total: 3,
            ..Default::default()
        }));
        assert!(app.thinking.active.is_some());
        assert!(
            app.thinking
                .active
                .as_ref()
                .unwrap()
                .content
                .contains("still thinking")
        );
    }

    #[test]
    fn empty_started_finished_leaves_no_thinking_ui() {
        let mut app = make_app();
        let before = app.raw_messages.len();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        assert!(app.thinking.active.is_some());
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        assert!(app.thinking.active.is_none());
        assert!(app.thinking.blocks.is_empty());
        assert_eq!(app.raw_messages.len(), before);
    }

    #[test]
    fn whitespace_only_delta_finished_leaves_no_thinking_block() {
        let mut app = make_app();
        let before = app.raw_messages.len();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "   ".into(),
        )));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        assert!(app.thinking.blocks.is_empty());
        assert!(app.thinking.active.is_none());
        assert_eq!(app.raw_messages.len(), before);
    }

    #[test]
    fn thinking_finished_keeps_the_existing_placeholder_index() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "done thinking\n".into(),
        )));
        let phys_idx = app.thinking.active.as_ref().unwrap().phys_idx;

        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));

        assert_eq!(app.thinking.blocks[0].phys_idx, phys_idx);
        assert!(app.thinking.active.is_none());
    }

    #[test]
    fn missing_thinking_started_creates_one_placeholder_not_source_rows() {
        let mut app = make_app();
        let before = app.raw_messages.len();

        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "first\nsecond".into(),
        )));

        assert_eq!(
            app.raw_messages.len(),
            before + crate::render::cells::thinking::thinking_visual_rows(2)
        );
        assert_eq!(
            app.thinking.active.as_ref().unwrap().display_tail().len(),
            2
        );
    }
}
