use crate::render::render_md::{format_table, is_horizontal_rule, render_markdown_tui};
use crate::widgets::state::*;
use crate::widgets::tool_widget::ToolWidget;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, ScrollbarState};
use std::time::Instant;
use tact_core::{AgentErrorKind, AgentUpdate, StepStatus, UserCommand};

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

impl App {
    pub(crate) fn handle_agent_update(&mut self, update: AgentUpdate) {
        self.dirty = true;
        // Close the previous thinking block: when any non-ThinkingChunk update arrives,
        // it means the LLM has finished the thinking phase and subsequent output
        // does not belong to the thinking region.
        if !matches!(update, AgentUpdate::ThinkingChunk(_)) {
            self.flush_and_close_thinking();
        }
        // Remove the loading placeholder on any content-producing update
        // (PlanGenerated is the one that inserts it, so skip that)
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
            // PlanGenerated ignore temp
            AgentUpdate::PlanGenerated(plan) => {
                // New task starts: flush leftover streaming lines
                self.flush_stream_pending();

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
                self.messages.push(Line::from(""));
                self.raw_messages.push(String::new());
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
                // total was set once by PlanGenerated and should not grow with each
                // tool call dispatch from execute_tool_call().
                // If we're not yet in Executing (e.g. no PlanGenerated), fall back
                // to a safe default.
                self.ensure_executing_status(idx);
                self.add_new_line();
                self.add_system_message(format!("  {}. {}", idx + 1, step.description));
                self.plan.scroll_state =
                    ScrollbarState::new(self.plan.steps.len().saturating_sub(1));
            }
            AgentUpdate::StepStarted(idx, tool_id) => {
                let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
                // Flush leftover streaming content first (especially the last thinking line),
                // ensuring all LLM output before tool execution is fully displayed.
                self.flush_stream_pending();
                if let Status::Executing {
                    current_step,
                    total,
                } = &mut self.status
                {
                    *current_step = idx;
                    // Ensure total is at least idx + 1, so the progress bar
                    // never overshoots 100%. This handles the case where
                    // there are more tool calls than plan steps (the plan
                    // had 3 high-level steps but 10 tool calls are dispatched).
                    if idx >= *total {
                        *total = idx + 1;
                    }
                }
                if let Some(step) = self.plan.steps.get(idx) {
                    let description = step.description.clone();
                    let msgs = self.msgs();
                    // Insert a blank line between content and tool invocation as visual separator
                    self.add_system_message(msgs.step_started_tmpl.replace("{}", &description));
                }
            }
            AgentUpdate::StepFinished(idx, tool_id, result) => {
                let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
                // Close the thinking block for the current step, preventing the next step's
                // ThinkingChunk from skipping the title because thinking_title_added is still true.
                self.flush_stream_pending();
                let msgs = self.msgs();
                let icon = match result.status {
                    StepStatus::Success => msgs.step_success_prefix,
                    StepStatus::Failed => msgs.step_fail_prefix,
                };
                let log_msg = if result.arg_summary.is_empty() {
                    msgs.step_finished_simple_tmpl
                        .replacen("{}", icon, 1)
                        .replacen("{}", &(idx + 1).to_string(), 1)
                        .replacen("{}", &result.tool, 1)
                } else {
                    msgs.step_finished_args_tmpl
                        .replacen("{}", icon, 1)
                        .replacen("{}", &(idx + 1).to_string(), 1)
                        .replacen("{}", &result.tool, 1)
                        .replacen("{}", &result.arg_summary, 1)
                };
                let bytes_str = match result.tool.as_str() {
                    "read_file" | "write_file" => result
                        .detail
                        .as_ref()
                        .map(|d| msgs.step_bytes_tmpl.replace("{}", &d.len().to_string()))
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let duration_str = result.duration_ms.map_or(String::new(), |ms| {
                    if ms < 1000 {
                        msgs.step_ms_tmpl.replace("{}", &ms.to_string())
                    } else {
                        format!(
                            "{}",
                            msgs.step_sec_tmpl
                                .replace("{}", &format!("{:.1}", ms as f64 / 1000.0))
                        )
                    }
                });
                let log_msg = format!("{}{}{}", log_msg, bytes_str, duration_str);
                self.add_system_message(log_msg);

                // Tool operations with detail output: use ToolWidget to produce a
                // ToolRenderOutput, then store it as a ToolBlock. phys_idx points
                // to the summary line so LogColumnRenderer can replace it with a
                // full ToolCell (summary + detail card).
                let tool_has_detail =
                    matches!(result.tool.as_str(), "write_file" | "read_file" | "bash");
                if tool_has_detail && result.detail.is_some() {
                    let msgs = self.msgs();
                    let tw = ToolWidget::from_step_result(idx, &result, &self.theme, &msgs);
                    let output = tw.build();

                    let placeholder_count = output.message_placeholder_rows();
                    // phys_idx points to the summary line pushed by add_system_message above.
                    let phys_idx = self.messages.len().saturating_sub(1);
                    for _ in 0..placeholder_count {
                        self.messages.push(Line::from(""));
                        self.raw_messages.push(String::new());
                    }
                    self.tool_blocks.push(ToolBlock { phys_idx, output });
                    self.log_scroll.state =
                        ScrollbarState::new(self.total_log_lines().saturating_sub(1));
                    if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal
                    {
                        self.log_scroll.offset = u16::MAX;
                    }
                    if !self.search.term.is_empty() {
                        self.update_search_matches();
                    }
                }

                // Store output preview in plan step for Plan panel display
                if let Some(step) = self.plan.steps.get_mut(idx) {
                    step.output = Some(result.message);
                }
            }
            AgentUpdate::StepFailed(idx, tool_id, error) => {
                let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
                self.flush_stream_pending();
                let msgs = self.msgs();
                self.add_system_message(
                    msgs.step_failed_tmpl
                        .replacen("{}", &(idx + 1).to_string(), 1)
                        .replacen("{}", &error, 1),
                );
                self.status = Status::Idle;
                self.task_start_time = None;
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
                self.task_start_time = None;
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
                        self.task_start_time = None;
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
                self.select.set(prompt, options, respond);
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
                    self.messages.push(Line::from(""));
                    self.raw_messages.push(String::new());
                    let separator_idx = self.messages.len() - 1;

                    self.messages.push(Line::from(Span::styled(
                        msgs.thinking_title.to_string(),
                        title_style,
                    )));
                    self.raw_messages.push(msgs.thinking_title.to_string());
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
                    self.messages
                        .push(Line::from(Span::styled(text.clone(), style)));
                    self.raw_messages.push(text);
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
                        self.messages
                            .push(Line::from(Span::styled(text.clone(), style)));
                        self.raw_messages.push(text);
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
                                let (styled, _) = render_markdown_tui(&code_text);
                                let placeholder_count = styled.len().min(MAX_CODE_PREVIEW) + 2; // +2 for card border
                                let placeholders: Vec<Line<'static>> =
                                    (0..placeholder_count).map(|_| Line::from("")).collect();
                                let raw_placeholders: Vec<String> =
                                    (0..placeholder_count).map(|_| String::new()).collect();
                                let _: Vec<_> = self
                                    .messages
                                    .splice(start_idx..stream_end, placeholders)
                                    .collect();
                                let _: Vec<_> = self
                                    .raw_messages
                                    .splice(start_idx..stream_end, raw_placeholders)
                                    .collect();
                                self.code_blocks.push(CodeBlock {
                                    start_idx,
                                    end_idx: start_idx + placeholder_count,
                                    lang,
                                    content: lines.join("\n"),
                                    styled,
                                });
                            } else {
                                self.messages.drain(start_idx..stream_end);
                                self.raw_messages.drain(start_idx..stream_end);
                            }
                        } else if !lines.is_empty() {
                            let code_text = format!("```{}\n{}\n```", lang, lines.join("\n"));
                            let (styled, raw) = render_markdown_tui(&code_text);
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
                        self.messages.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(Color::DarkGray).bg(CODE_BG)),
                            Span::styled(display_line, Style::default().fg(CODE_FG).bg(CODE_BG)),
                        ]));
                        self.raw_messages.push(line);
                        self.stream.code_block_line_count += 1;
                    } else if is_code_fence {
                        // Open new code block: flush pending content first
                        if !self.stream.paragraph.is_empty() {
                            let paragraph = std::mem::take(&mut self.stream.paragraph);
                            let (styled, raw) = render_markdown_tui(&paragraph);
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
                            self.messages.push(styled_line);
                            self.raw_messages.push(raw_line);
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
                        self.messages.push(Line::from(Span::styled(
                            header_text.clone(),
                            Style::default().fg(Color::DarkGray).bg(CODE_BG),
                        )));
                        self.raw_messages.push(format!("```{}", lang));
                    } else {
                        // Regular line handling
                        let is_table_line = trimmed.starts_with('|');
                        let is_blank = trimmed.is_empty();
                        let is_hr = is_horizontal_rule(&line);

                        if is_table_line {
                            if !self.stream.paragraph.is_empty() {
                                let paragraph = std::mem::take(&mut self.stream.paragraph);
                                let (styled, raw) = render_markdown_tui(&paragraph);
                                completed.extend(styled.into_iter().zip(raw));
                            }
                            self.stream.table_buffer.push(line);
                        } else if is_blank || is_hr {
                            if !self.stream.paragraph.is_empty() {
                                let paragraph = std::mem::take(&mut self.stream.paragraph);
                                let (styled, raw) = render_markdown_tui(&paragraph);
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
                    self.messages.push(styled_line);
                    self.raw_messages.push(raw_line);
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
