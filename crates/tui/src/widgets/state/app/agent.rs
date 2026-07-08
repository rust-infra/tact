use crate::render::render_md::{format_table, is_horizontal_rule, render_markdown_tui};
use crate::widgets::state::*;
use crate::widgets::tool_widget::{ToolPhase, ToolWidget};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, ScrollbarState};
use std::time::Instant;
use tact_protocol::{AgentErrorKind, AgentUpdate};

const CODE_BG: Color = Color::Rgb(30, 35, 50);
const CODE_FG: Color = Color::Rgb(200, 200, 210);
const STREAMING_INDICATOR: &str = " ▌";

fn resolve_step_idx(steps: &[PlanStep], tool_id: &str, idx: usize) -> usize {
    if !tool_id.is_empty() {
        if let Some(found) = steps.iter().position(|s| s.tool_id == tool_id) {
            return found;
        }
    }
    idx
}

fn plan_step_arg_full(step: &PlanStep) -> String {
    match step.tool.as_str() {
        "read_file" | "write_file" => step.args.get("path").cloned().unwrap_or_default(),
        "run_command" | "bash" | "shell" => step.args.get("command").cloned().unwrap_or_default(),
        _ => {
            if step.args.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&step.args).unwrap_or_default()
            }
        }
    }
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

    #[allow(deprecated)]
    pub(crate) fn handle_agent_update(&mut self, update: AgentUpdate) {
        self.dirty = true;
        // Close the previous thinking block: when any non-ThinkingChunk update arrives,
        // it means the LLM has finished the thinking phase and subsequent output
        // does not belong to the thinking region.
        if !matches!(update, AgentUpdate::ThinkingChunk(_)) {
            self.flush_and_close_thinking();
        }
        // Remove the loading placeholder on any content-producing update.
        // PlanGenerated is a legacy path that inserts it, so skip that variant.
        // Metadata-only updates (TokenUsage, Balance, ModelInfo) should NOT
        // remove the placeholder since they don't produce visible content.
        match &update {
            AgentUpdate::PlanGenerated(_)
            | AgentUpdate::TokenUsage { .. }
            | AgentUpdate::Balance(_)
            | AgentUpdate::ModelInfo(_) => {
                // These don't remove the loading placeholder:
                // - PlanGenerated: we just inserted it
                // - TokenUsage / Balance / ModelInfo: metadata only, no content
            }
            _ => {
                self.remove_loading_placeholder();
            }
        }
        match update {
            // Legacy PlanGenerated path. Current agent code drives the plan panel
            // through StepAdded / StepStarted and does not emit this variant.
            AgentUpdate::PlanGenerated(plan) => {
                // New task starts: flush leftover streaming lines
                self.flush_stream_pending();
                self.cancel_all_active_tools();

                let plan_len = plan.len();
                self.plan.steps = plan;
                self.plan.collapsed = vec![false; plan_len];
                self.plan.selected = 0;
                self.plan.list_state =
                    ListState::default().with_selected(if plan_len > 0 { Some(0) } else { None });
                self.status = Status::Executing {
                    current_step: 0,
                    total: plan_len,
                };
                let msgs = self.msgs();
                let plan_messages: Vec<String> = self
                    .plan
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(i, step)| {
                        msgs.plan_step_tmpl
                            .replacen("{}", &(i + 1).to_string(), 1)
                            .replacen("{}", &step.description, 1)
                    })
                    .collect();
                self.add_system_message(format!(
                    "{}",
                    msgs.plan_generated_tmpl
                        .replace("{}", &plan_len.to_string())
                ));
                for msg in plan_messages {
                    self.add_system_message(msg);
                }
                self.plan.scroll_state = ScrollbarState::new(plan_len.saturating_sub(1));

                // Insert a loading placeholder line for the spinner animation
                self.append_blank(RawMessageType::SysTool);
                self.loading_idx = Some(self.messages.len() - 1);
            }
            AgentUpdate::StepAdded(step) => {
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
                // Current agent runs may never send PlanGenerated, so ensure there
                // is an Executing status before StepStarted arrives.
                self.ensure_executing_status(idx);
                self.plan.scroll_state =
                    ScrollbarState::new(self.plan.steps.len().saturating_sub(1));
            }
            AgentUpdate::StepStarted(idx, tool_id, tool_name, arg_summary) => {
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
                let full_arg = self
                    .plan
                    .steps
                    .get(idx)
                    .map(plan_step_arg_full)
                    .unwrap_or_default();
                let output = ToolWidget::new(&self.theme, &msgs)
                    .with_tool(tool_name)
                    .with_arg_summary(arg_summary)
                    .with_arg_full(full_arg)
                    .with_step_index(idx)
                    .with_phase(ToolPhase::Running)
                    .with_duration_us(0)
                    .build();
                let phys_idx = self.push_tool_placeholder_rows(&output);
                self.tools.active.push(ActiveToolBlock {
                    phys_idx,
                    tool_id,
                    output,
                    started_at: Instant::now(),
                });
                self.refresh_tool_log_scroll();
            }
            AgentUpdate::StepFinished(idx, tool_id, result) => {
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
            AgentUpdate::StepFailed(idx, tool_id, error) => {
                let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
                self.flush_stream_pending();
                if let Some(active) = self
                    .tools
                    .active
                    .iter()
                    .find(|a| a.tool_id == tool_id)
                {
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
            // Handle cases requiring user approval
            AgentUpdate::NeedApproval(prompt, step_idx, tx) => {
                // Close the active thinking block first, preventing approval messages
                // from being captured inside a collapsed region.
                self.flush_stream_pending();
                let prompt_clone = prompt.clone();
                self.status = Status::WaitingForUser {
                    prompt,
                    step_index: step_idx,
                    approval_tx: tx,
                };
                self.input_mode = InputMode::Normal;
                let msgs = self.msgs();
                self.add_system_message(msgs.need_approval_tmpl.replace("{}", &prompt_clone));
            }
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
            // Error handling
            AgentUpdate::Error(kind) => {
                match kind {
                    AgentErrorKind::BalanceNotSupported => {
                        self.balance_info = None;
                        // self.flash_msg = Some((
                        //     "Balance query not supported for this model".to_string(),
                        //     std::time::Instant::now(),
                        // ));
                        self.dirty = true;
                    }
                    AgentErrorKind::BalanceQueryFailed(err) => {
                        self.balance_info = None;
                        self.flash_msg = Some((
                            format!("Balance query failed: {}", err),
                            std::time::Instant::now(),
                        ));
                        self.dirty = true;
                    }
                    AgentErrorKind::Other(msg) => {
                        // Fatal error: flush leftover streaming lines
                        self.flush_stream_pending();
                        let msgs = self.msgs();
                        self.add_system_message(msgs.error_tmpl.replace("{}", &msg));
                        self.status = Status::Idle;
                        self.freeze_last_prompt_cost();
                    }
                }
            }
            // Update token usage info
            AgentUpdate::TokenUsage {
                prompt,
                completion,
                total,
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
                reasoning_tokens,
            } => {
                self.status_bar.token_prompt = prompt;
                self.status_bar.token_completion = completion;
                self.status_bar.token_total = total;
                self.status_bar.token_cache_hit = prompt_cache_hit_tokens;
                self.status_bar.token_cache_miss = prompt_cache_miss_tokens;
                self.status_bar.token_reasoning = reasoning_tokens;
            }
            // Update balance info
            AgentUpdate::Balance(info) => {
                self.balance_info = Some(info.clone());
            }
            // Update model info
            AgentUpdate::ModelInfo(params) => {
                self.status_bar.model_name = params.model;
                self.status_bar.model_max_tokens = params.max_tokens;
                self.status_bar.model_thinking_budget = params.thinking_budget;
            }
            // Add system message
            AgentUpdate::Info(msg) => {
                self.add_system_message(msg);
            }
            AgentUpdate::RequestSelect {
                prompt,
                options,
                respond,
            } => {
                self.select.set(prompt, options, respond, false);
                self.input_mode = InputMode::Select;
            }
            AgentUpdate::ThinkingChunk(text) => {
                self.thinking.buffer.push_str(&text);
                let msgs = self.msgs();

                // Add a title line on first thinking chunk
                if !self.thinking.title_added {
                    let title_style = Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC)
                        .bg(Color::Rgb(35, 35, 45));
                    // Insert a blank isolation line before the title to establish visual
                    // separation before collapsing
                    self.append_blank(RawMessageType::LLMThinking);
                    let separator_idx = self.messages.len() - 1;

                    self.append_msg(
                        Line::from(Span::styled(
                            msgs.thinking_title.to_string(),
                            title_style,
                        )),
                        msgs.thinking_title.to_string(),
                        RawMessageType::LLMThinking,
                    );
                    self.thinking.title_added = true;
                    self.thinking.active_start = Some(separator_idx);
                    self.thinking.thinking_start = Some(Instant::now());
                }

                // Line-level buffering: extract complete lines for real-time display
                let style = Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC)
                    .bg(Color::Rgb(35, 35, 45));
                while let Some(idx) = self.thinking.buffer.find('\n') {
                    let line = self.thinking.buffer[..idx].to_string();
                    self.thinking.buffer = self.thinking.buffer[idx + 1..].to_string();
                    let text = if line.is_empty() {
                        String::new()
                    } else {
                        msgs.thinking_line_prefix.replace("{}", &line).to_string()
                    };
                    self.append_msg(
                        Line::from(Span::styled(text.clone(), style)),
                        text,
                        RawMessageType::LLMThinking,
                    );
                    self.thinking.active_end = Some(self.messages.len() - 1);
                }

                self.log_scroll.state =
                    ScrollbarState::new(self.total_log_lines().saturating_sub(1));
                if !self.search.term.is_empty() {
                    self.update_search_matches();
                }
                // u16::MAX is correctly clipped by render_log_panel based on visual line count
                self.log_scroll.offset = u16::MAX;
            }
            AgentUpdate::StreamChunk(text) => {
                self.ensure_gap_after_tools();
                // Flush leftover thinking lines (the last line without trailing newline)
                // Note: the thinking block has already been closed by the gate at
                // the entry of handle_agent_update.
                if !self.thinking.buffer.is_empty() {
                    let style = Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC)
                        .bg(Color::Rgb(35, 35, 45));
                    let text = if self.thinking.buffer.trim().is_empty() {
                        String::new()
                    } else {
                        format!("│ {}", self.thinking.buffer)
                    };
                    if !text.is_empty() {
                        self.append_msg(
                            Line::from(Span::styled(text.clone(), style)),
                            text,
                            RawMessageType::LLMThinking,
                        );
                    }
                    self.thinking.buffer.clear();
                }
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
                        if self.stream.code_block_line_count > 1 {
                            if let Some(prev_raw) = self.raw_messages.get_mut(prev_idx) {
                                if prev_raw.ends_with(STREAMING_INDICATOR) {
                                    let clean =
                                        prev_raw.trim_end_matches(STREAMING_INDICATOR).to_string();
                                    *prev_raw = clean.clone();
                                    self.messages[prev_idx] = Line::from(vec![
                                        Span::styled(
                                            "│ ",
                                            Style::default().fg(Color::DarkGray).bg(CODE_BG),
                                        ),
                                        Span::styled(
                                            clean,
                                            Style::default().fg(CODE_FG).bg(CODE_BG),
                                        ),
                                    ]);
                                }
                            }
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
                            let (styled, raw) =
                                format_table(&self.stream.table_buffer, &self.theme);
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
                                let (styled, raw) =
                                    format_table(&self.stream.table_buffer, &self.theme);
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
                                let (styled, raw) =
                                    format_table(&self.stream.table_buffer, &self.theme);
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

                self.log_scroll.state =
                    ScrollbarState::new(self.total_log_lines().saturating_sub(1));
                if !self.search.term.is_empty() {
                    self.update_search_matches();
                }
                // Auto-scroll to bottom (u16::MAX clipped by render_log_panel to visual line count)
                self.log_scroll.offset = u16::MAX;
            }
        }
        // Unified tail scroll state refresh, covering cases where helpers like
        // flush_and_close_thinking / flush_stream_pending inserted messages without
        // updating scroll (most arms call add_system_message independently,
        // StreamChunk / ThinkingChunk also update separately; this redundant call is
        // cheap and harmless).
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        if !self.search.term.is_empty() {
            self.update_search_matches();
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use crate::widgets::state::{App, Status};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus, AgentErrorKind};
    use tokio::sync::mpsc::unbounded_channel;

    fn make_app() -> App {
        let (_agent_tx, agent_rx) = unbounded_channel();
        let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        App::new(
            agent_rx,
            user_cmd_tx,
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
        )
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
        app.handle_agent_update(AgentUpdate::StepStarted(
            0,
            "tool_read_1".into(),
            "read_file".into(),
            "main.rs".into(),
        ));
        app.handle_agent_update(AgentUpdate::StepFinished(
            0,
            "tool_read_1".into(),
            StepResult {
                tool: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("file body".into()),
                duration_us: Some(1),
                permission_label: None,
            },
        ));

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
        app.handle_agent_update(AgentUpdate::StepFailed(
            0,
            "tool_read_1".into(),
            "file not found".into(),
        ));
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
            app.raw_messages.last().is_some_and(|m| m.contains("Cancelling"))
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
        app.handle_agent_update(AgentUpdate::TokenUsage {
            prompt: 100,
            completion: 50,
            total: 150,
            prompt_cache_hit_tokens: 10,
            prompt_cache_miss_tokens: 90,
            reasoning_tokens: 5,
        });
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
        });
        assert!(matches!(app.input_mode, InputMode::Select));
        assert!(app.select.prompt.contains("Allow bash"));
    }

    #[test]
    fn thinking_chunk_flushes_on_stream() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk("reasoning line".into()));
        assert!(!app.thinking.buffer.is_empty());
        app.handle_agent_update(AgentUpdate::StreamChunk("final answer".into()));
        assert!(app.thinking.buffer.is_empty());
    }

    #[test]
    fn model_info_updates_status_bar() {
        use tact_protocol::ModelCallParams;

        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ModelInfo(ModelCallParams {
            model: "mock-model".into(),
            max_tokens: 4096,
            thinking_budget: Some(0),
            reasoning_effort: None,
            extra_body: None,
        }));
        assert_eq!(app.status_bar.model_name, "mock-model");
        assert_eq!(app.status_bar.model_max_tokens, 4096);
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
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::Error(AgentErrorKind::BalanceQueryFailed(
            "network down".into(),
        )));
        assert!(
            app.flash_msg
                .as_ref()
                .is_some_and(|(msg, _)| msg.contains("network down"))
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
        app.handle_agent_update(AgentUpdate::StepStarted(
            0,
            "t1".into(),
            "read_file".into(),
            "a.rs".into(),
        ));
        assert!(matches!(app.status, Status::Executing { .. }));
        app.handle_agent_update(AgentUpdate::StepFinished(
            0,
            "t1".into(),
            StepResult {
                tool: "read_file".into(),
                arg_summary: "a.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: None,
                duration_us: Some(1),
                permission_label: None,
            },
        ));
        assert!(
            !matches!(app.status, Status::Done),
            "single step finish should not mark task done"
        );
    }

    #[test]
    fn balance_not_supported_clears_balance_info() {
        let mut app = make_app();
        app.balance_info = Some(tact_protocol::BalanceInfo {
            is_available: true,
            balance_infos: vec![],
        });
        app.handle_agent_update(AgentUpdate::Error(AgentErrorKind::BalanceNotSupported));
        assert!(app.balance_info.is_none());
    }

    #[test]
    fn thinking_chunks_accumulate_before_non_thinking_update() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk("part1 ".into()));
        app.handle_agent_update(AgentUpdate::ThinkingChunk("part2".into()));
        assert!(app.thinking.buffer.contains("part1"));
        assert!(app.thinking.buffer.contains("part2"));
        app.handle_agent_update(AgentUpdate::Info("done thinking".into()));
        assert!(app.thinking.buffer.is_empty());
    }
}
