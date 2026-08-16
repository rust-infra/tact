use std::time::Instant;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    widgets::ScrollbarState,
};
use tact::plugin::{PluginEvent, PluginOperation, PluginResult};
use tact_protocol::{
    AccountError, AccountUpdate, AgentErrorKind, AgentUpdate, PlanStep, StepResult, TaskSnapshot,
    TasksChangeReason, ThinkingChunk, TokenUsageInfo, ToolOutputBuffer, ToolOutputChunk,
};

use crate::{
    render::render_md::{
        format_table, is_horizontal_rule, render_markdown_tui, render_mermaid_block,
        render_plain_markdown,
    },
    widgets::{
        state::*,
        tool_widget::{ToolPhase, ToolWidget},
    },
};

const CODE_BG: Color = Color::Rgb(30, 35, 50);
const CODE_FG: Color = Color::Rgb(200, 200, 210);
const STREAMING_INDICATOR: &str = " ▌";
const MAX_PLUGIN_FAILURE_DETAIL_CHARS: usize = 512;

fn sanitize_plugin_failure_detail(detail: &str) -> String {
    let mut sanitized: String = detail
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(MAX_PLUGIN_FAILURE_DETAIL_CHARS + 1)
        .collect();

    if sanitized.chars().count() > MAX_PLUGIN_FAILURE_DETAIL_CHARS {
        sanitized = sanitized
            .chars()
            .take(MAX_PLUGIN_FAILURE_DETAIL_CHARS)
            .collect();
        sanitized.push_str("...");
    }

    sanitized
}

fn replace_two(template: &str, first: &str, second: &str) -> String {
    template.replacen("{}", first, 1).replacen("{}", second, 1)
}

fn format_plugin_result(messages: &crate::i18n::Messages, result: &PluginResult) -> String {
    match result {
        PluginResult::Installed {
            plugin,
            marketplace,
        } => replace_two(messages.plugin_installed_tmpl, plugin, marketplace),
        // Rendered as a titled table by `App::show_plugin_list`; plain fallback only.
        PluginResult::ListedInstalled { .. } => messages.plugin_list_empty.to_owned(),
        PluginResult::Reloaded { count } => messages
            .plugin_reloaded_tmpl
            .replace("{}", &count.to_string()),
        PluginResult::MarketplaceAdded { marketplace } => {
            messages.marketplace_added_tmpl.replace("{}", marketplace)
        }
        // Rendered as a titled table by `App::show_marketplace_list`; plain fallback only.
        PluginResult::ListedMarketplaces { .. } => messages.marketplace_list_empty.to_owned(),
        PluginResult::MarketplaceUpdated { marketplace, count } => replace_two(
            messages.marketplace_updated_tmpl,
            marketplace,
            &count.to_string(),
        ),
        PluginResult::MarketplaceRemoved { marketplace } => {
            messages.marketplace_removed_tmpl.replace("{}", marketplace)
        }
    }
}

fn plugin_operation_label(
    messages: &crate::i18n::Messages,
    operation: &PluginOperation,
) -> &'static str {
    match operation {
        PluginOperation::Install { .. } => messages.plugin_operation_install,
        PluginOperation::List => messages.plugin_operation_list,
        PluginOperation::Reload => messages.plugin_operation_reload,
        PluginOperation::MarketplaceAdd => messages.plugin_operation_marketplace_add,
        PluginOperation::MarketplaceList => messages.plugin_operation_marketplace_list,
        PluginOperation::MarketplaceUpdate { .. } => messages.plugin_operation_marketplace_update,
        PluginOperation::MarketplaceRemove { .. } => messages.plugin_operation_marketplace_remove,
    }
}

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
    /// Short elapsed label for status bar during active runs.
    pub(crate) fn format_task_elapsed(&self) -> String {
        let start = match self.task_start_time {
            Some(s) => s,
            None => return String::new(),
        };
        let secs = chrono::Local::now()
            .signed_duration_since(start)
            .num_seconds()
            .max(0);
        let mm_ss = format!("{:02}:{:02}", secs / 60, secs % 60);
        format!("⏱ {} {}", self.msgs().bottom_elapsed, mm_ss)
    }

    fn freeze_last_prompt_cost(&mut self) {
        if let Some(start) = self.task_start_time.take() {
            self.last_prompt_elapsed_secs = Some(elapsed_secs_since(start));
        }
    }

    pub(crate) fn handle_agent_update(&mut self, update: AgentUpdate) {
        self.dirty = true;

        // Safety net: close an open thinking region on content-producing updates
        // that are not ThinkingChunk. Explicit ThinkingChunk::Finished is preferred;
        // TokenUsage / ModelInfo / ToolMeta must not close the region (they can
        // arrive mid-stream).
        match &update {
            AgentUpdate::ThinkingChunk(_)
            | AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolMeta { .. }
            | AgentUpdate::ToolProgress { .. } => {}
            _ => {
                self.flush_and_close_thinking();
            }
        }
        // Remove the loading placeholder on any content-producing update.
        // Metadata-only updates (TokenUsage, Balance, UsageQuota, ModelInfo,
        // ToolMeta) should NOT remove the placeholder since they don't produce
        // visible content.
        match &update {
            AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolMeta { .. }
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
                presentation,
            } => self.on_step_started(idx, tool_id, tool_name, arg_summary, arg_full, presentation),
            AgentUpdate::StepFinished {
                idx,
                tool_id,
                result,
            } => self.on_step_finished(idx, tool_id, result),
            AgentUpdate::StepFailed {
                idx,
                tool_id,
                arg_summary,
                error,
            } => self.on_step_failed(idx, tool_id, arg_summary, error),
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
                    self.scroll_log_to_bottom();
                }
                self.status = Status::Done;
                self.freeze_last_prompt_cost();
                self.task_done_time = Some(chrono::Local::now());
                // Task stats block: elapsed is frozen by add_task_end_separator,
                // token/model snapshots live in the status bar.
                self.add_task_stats_block();
            }
            AgentUpdate::TaskCancelled => {
                // Cancel exits without TaskComplete; must leave Planning/Executing
                // or Enter keeps flashing input_busy_msg.
                self.flush_stream_pending();
                self.add_task_end_separator();
                if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
                    self.scroll_log_to_bottom();
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
            // Whole-Markdown notice, rendered as a single MarkdownCell
            AgentUpdate::MdInfo(msg) => {
                self.append_markdown(msg);
            }
            AgentUpdate::SessionStats(stats_text) => {
                self.system_prompt_popup = Some(SystemPromptPopup {
                    title: "Session Statistics".to_string(),
                    source: stats_text,
                    scroll: 0,
                });
                self.input_mode = InputMode::Normal;
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
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
            } => self.on_tool_meta(&tool_id, model, token_usage),
            AgentUpdate::BackgroundTaskFinished {
                tool_id,
                success,
                message,
                output,
            } => self.on_background_task_finished(&tool_id, success, &message, &output),
            AgentUpdate::StreamChunk(text) => self.apply_stream_chunk(text),
            AgentUpdate::TasksChanged { tasks, reason } => {
                self.on_tasks_changed(tasks, reason);
            }
        }
        // Unified tail scroll state refresh, covering cases where helpers like
        // flush_and_close_thinking / flush_stream_pending inserted messages without
        // updating scroll (most arms call add_system_message independently,
        // StreamChunk / ThinkingChunk also update separately; this redundant call is
        // cheap and harmless).
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
    }

    /// Snapshot changes only drive the sticky panel; the Log already shows the
    /// originating `task_*` tool row, so no extra system message is appended.
    fn on_tasks_changed(&mut self, tasks: Vec<TaskSnapshot>, _: TasksChangeReason) {
        let was_visible = self.task_panel.visible;
        self.task_panel.apply_snapshot(tasks);
        if self.task_panel.visible && !was_visible {
            self.task_panel.expanded = true;
        }
        // Keep an open /tasks-dag popup in sync: its lines were rendered when
        // the popup opened and would otherwise never show later task changes
        // (the render loop only re-renders on width changes).
        if self.task_dag_popup.is_some() {
            let width = self
                .task_dag_popup
                .as_ref()
                .map_or(crate::widgets::state::DEFAULT_DAG_RENDER_WIDTH, |p| {
                    p.render_width
                });
            let (source, lines) =
                render_task_dag_lines(&self.task_panel.snapshot, &self.theme, width);
            if let Some(p) = self.task_dag_popup.as_mut() {
                p.lines = lines;
                p.mermaid_source = source;
            }
        }
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
        // Don't change current_step or total — the step hasn't started yet.
        // Ensure there is an Executing status before StepStarted arrives.
        self.ensure_executing_status(idx);
    }

    fn on_step_started(
        &mut self,
        idx: usize,
        tool_id: String,
        tool_name: String,
        arg_summary: String,
        arg_full: String,
        presentation: tact_protocol::ToolPresentationInfo,
    ) {
        let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
        self.flush_stream_pending();
        // Same tool_id restarting without a finish: drop stale placeholder rows.
        self.cancel_active_tool(&tool_id);
        // Full live output for subagents (based on presentation metadata, not tool name).
        let is_subagent = matches!(
            &presentation.popup,
            tact_protocol::ToolPopupKind::SubagentTranscript
        );
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
            live_output: if is_subagent {
                ToolOutputBuffer::new_full(50_000)
            } else {
                ToolOutputBuffer::new(50_000)
            },
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
        let was_pinned = self.is_log_pinned_to_bottom();
        self.tools.active[pos].live_output.push_chunks(chunks);
        if self.tools.active[pos].live_output.logical_line_count() == 0 {
            return;
        }

        let msgs = self.msgs();
        let step_idx = resolve_step_idx(&self.plan.steps, tool_id, 0);
        let (phys_idx, old_rows, output) = {
            let active = &self.tools.active[pos];
            // Preserve subagent metadata when rebuilding the output.
            let output = ToolWidget::new(&self.theme, &msgs)
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
        self.resize_tool_placeholder_rows(phys_idx, old_rows, new_rows);
        self.tools.active[pos].output = output;
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        if was_pinned {
            self.scroll_log_to_bottom();
        }
    }

    fn on_tool_meta(
        &mut self,
        tool_id: &str,
        model: Option<String>,
        token_usage: Option<TokenUsageInfo>,
    ) {
        let Some(pos) = self
            .tools
            .active
            .iter()
            .position(|active| active.tool_id == tool_id)
        else {
            return;
        };
        let active = &mut self.tools.active[pos];
        if let Some(m) = model {
            active.output.subagent_model = Some(m);
        }
        if let Some(t) = token_usage {
            active.output.subagent_tokens = Some(t);
        }
        self.dirty = true;
    }

    fn on_step_finished(&mut self, idx: usize, tool_id: String, result: StepResult) {
        let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
        self.flush_stream_pending();
        let msgs = self.msgs();

        // Keep-live tools (e.g. `background_run`) return immediately but their
        // card keeps streaming: skip finalization here; a later
        // `AgentUpdate::BackgroundTaskFinished` closes the card with the real
        // outcome. The plan step is still recorded as done (the invocation did
        // succeed at "started").
        if result.presentation.keep_live {
            if let Some(step) = self.plan.steps.get_mut(idx) {
                step.output = Some(result.message);
            }
            return;
        }

        let is_subagent = matches!(
            result.presentation.popup,
            tact_protocol::ToolPopupKind::SubagentTranscript
        );
        let mut output = ToolWidget::from_step_result(&result, &self.theme, &msgs)
            .with_step_index(idx)
            .build();

        // Subagent: live output holds the full conversation; detail_full would
        // otherwise only keep the final summary. Take it before the active block
        // is removed so the popup always shows the complete conversation.
        // Also carry over subagent metadata (model, tokens) so the completed
        // tool card header continues to show them.
        if is_subagent
            && let Some(active) = self.tools.active.iter_mut().find(|a| a.tool_id == tool_id)
        {
            let full_text = active.live_output.take_full_detail();
            if !full_text.is_empty() {
                output.detail_total_lines = full_text.lines().count();
                output.detail_full = Some(full_text);
            }
            output.subagent_model = active.output.subagent_model.take();
            output.subagent_tokens = active.output.subagent_tokens.take();
        }

        self.finalize_tool_block(&tool_id, output);

        if let Some(step) = self.plan.steps.get_mut(idx) {
            step.output = Some(result.message);
        }
    }

    /// Finalize a tool card that stayed live after its invocation returned
    /// (see [`ToolPresentationInfo::keep_live`]): the background task just
    /// finished, so render the real ✓/✗ outcome, duration, and final output.
    fn on_background_task_finished(
        &mut self,
        tool_id: &str,
        success: bool,
        message: &str,
        output: &str,
    ) {
        let Some(pos) = self
            .tools
            .active
            .iter()
            .position(|active| active.tool_id == tool_id)
        else {
            // The live card is gone (e.g. a fresh process after restart);
            // surface the outcome as a system message instead.
            let prefix = if success { "✓" } else { "✗" };
            self.add_system_message(format!("{prefix} {message}"));
            return;
        };
        let active = &self.tools.active[pos];
        let elapsed_us = active.started_at.elapsed().as_micros() as u64;
        let tool_name = active.output.tool_name.clone();
        let arg_summary = active.output.arg_summary.clone();
        let arg_full = active.output.arg_full.clone();
        let step_idx = resolve_step_idx(&self.plan.steps, tool_id, 0);
        let msgs = self.msgs();
        let mut widget = ToolWidget::new(&self.theme, &msgs)
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
            .with_detail(output.to_string());
        if !success {
            widget = widget.with_message(message.to_string());
        }
        self.finalize_tool_block(tool_id, widget.build());

        if let Some(step) = self.plan.steps.get_mut(step_idx) {
            step.output = Some(message.to_string());
        }
    }

    fn on_step_failed(&mut self, idx: usize, tool_id: String, arg_summary: String, error: String) {
        let idx = resolve_step_idx(&self.plan.steps, &tool_id, idx);
        self.flush_stream_pending();
        if let Some(active) = self.tools.active.iter().find(|a| a.tool_id == tool_id) {
            let elapsed_us = active.started_at.elapsed().as_micros() as u64;
            let tool_name = active.output.tool_name.clone();
            // Prefer the summary carried by the failure (e.g. a web-search
            // query that was only populated at `done`), falling back to the
            // `StepStarted` value so regular tool failures keep their title.
            let arg_summary = if arg_summary.is_empty() {
                active.output.arg_summary.clone()
            } else {
                arg_summary
            };
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

    /// Finalize a buffered stream code block — either closed by a fence inside
    /// `apply_stream_chunk`, or cut off by the stream ending in
    /// `flush_stream_pending`.
    ///
    /// `closed` reports whether a closing fence was actually seen. Valid,
    /// *closed* Mermaid is spliced directly into the log as diagram lines and
    /// never becomes a `CodeBlock` card. Every other block — ordinary code,
    /// invalid Mermaid, or an interrupted (unclosed) Mermaid fence — keeps
    /// the existing code-card overlay fallback (blank placeholder region +
    /// `CodeBlock` entry) so no source is dropped. The fallback styled preview
    /// is rendered through the plain (non-Mermaid) code path exactly once, so
    /// a reconstructed fallback fence is never re-routed through the Mermaid
    /// renderer, while the card keeps the original `lang` metadata and raw
    /// content. Resets `code_block_is_mermaid` once the buffered block is
    /// finalized.
    pub(crate) fn finish_stream_code_block(
        &mut self,
        lang: String,
        lines: Vec<String>,
        start_idx: Option<usize>,
        stream_end: usize,
        closed: bool,
    ) {
        let is_mermaid = self.stream.code_block_is_mermaid;
        self.stream.code_block_is_mermaid = false;

        if lines.is_empty() {
            if let Some(start) = start_idx {
                self.drain_msgs(start..stream_end);
            }
            return;
        }

        if closed
            && is_mermaid
            && let Some(diagram) = render_mermaid_block(
                &lines.join("\n"),
                &self.theme,
                self.log_scroll.width.max(1) as usize,
            )
        {
            let source = lines.join("\n");
            let raw = diagram.iter().map(|l| l.to_string()).collect::<Vec<_>>();
            let row_count = diagram.len();
            let start = match start_idx {
                Some(start) => {
                    self.splice_msgs(start..stream_end, diagram, raw, RawMessageType::LLM);
                    start
                }
                None => {
                    let start = self.messages.len();
                    self.extend_msgs(diagram, raw, RawMessageType::LLM);
                    start
                }
            };
            self.mermaid_blocks
                .push(crate::widgets::state::MermaidBlock {
                    start_idx: start,
                    end_idx: start + row_count,
                    source,
                });
            return;
        }

        // Existing code-card fallback: replace the streaming placeholder rows
        // with a sized blank region and store a CodeBlock overlay for card
        // rendering. Without a recorded placeholder range, append the rendered
        // content directly instead.
        //
        // The styled preview is rendered through the plain (non-Mermaid) code
        // path exactly once: re-routing the reconstructed fence through the
        // Mermaid router could draw a diagram for a fence that never closed
        // (or re-parse an invalid one), and a nested literal ```mermaid line
        // inside the buffered source must stay code content. The card keeps
        // the original `lang` metadata and raw `content`.
        let preview_source = format!("```{lang}\n{}\n```", lines.join("\n"));
        const MAX_CODE_PREVIEW: usize = 30;
        let (styled, raw) = render_plain_markdown(&preview_source, &self.theme, None);
        match start_idx {
            Some(start) => {
                let placeholder_count = styled.len().min(MAX_CODE_PREVIEW) + 2; // +2 for card border
                let placeholders: Vec<Line<'static>> =
                    (0..placeholder_count).map(|_| Line::from("")).collect();
                let raw_placeholders: Vec<String> =
                    (0..placeholder_count).map(|_| String::new()).collect();
                self.splice_msgs(
                    start..stream_end,
                    placeholders,
                    raw_placeholders,
                    RawMessageType::LLM,
                );
                self.code_blocks.push(CodeBlock {
                    start_idx: start,
                    end_idx: start + placeholder_count,
                    lang,
                    content: lines.join("\n"),
                    styled,
                });
            }
            None => {
                self.extend_msgs(styled, raw, RawMessageType::LLM);
            }
        }
    }

    fn apply_stream_chunk(&mut self, text: String) {
        self.ensure_gap_after_user_message();
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
                // Completed: finalize the buffered block — valid Mermaid is
                // spliced in as diagram lines, everything else becomes a
                // CodeBlock card (see finish_stream_code_block).
                let lang = std::mem::take(&mut self.stream.code_block_lang);
                let lines = std::mem::take(&mut self.stream.code_block_buffer);
                let start_idx = self.stream.code_block_start_idx.take();
                let stream_end = start_idx
                    .map(|s| s + self.stream.code_block_line_count)
                    .unwrap_or(0);
                self.finish_stream_code_block(lang, lines, start_idx, stream_end, true);
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
                let lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();

                // If an empty-language fence appears immediately after an
                // in-progress markdown paragraph/list, keep it in normal
                // markdown flow instead of promoting it into a standalone code
                // card. This avoids surprising card extraction for malformed or
                // explanatory fence snippets embedded in prose.
                if lang.is_empty() && !self.stream.paragraph.is_empty() {
                    if !self.stream.table_buffer.is_empty() {
                        let (styled, raw) = format_table(
                            &self.stream.table_buffer,
                            &self.theme,
                            Some(self.log_scroll.width as usize),
                        );
                        completed.extend(styled.into_iter().zip(raw));
                        self.stream.table_buffer.clear();
                    }
                    self.stream.paragraph.push('\n');
                    self.stream.paragraph.push_str(&line);
                    continue;
                }

                // Open new code block: flush pending content first
                if !self.stream.paragraph.is_empty() {
                    let paragraph = std::mem::take(&mut self.stream.paragraph);
                    let (styled, raw) = render_markdown_tui(&paragraph, &self.theme);
                    completed.extend(styled.into_iter().zip(raw));
                }
                if !self.stream.table_buffer.is_empty() {
                    let (styled, raw) = format_table(
                        &self.stream.table_buffer,
                        &self.theme,
                        Some(self.log_scroll.width as usize),
                    );
                    completed.extend(styled.into_iter().zip(raw));
                    self.stream.table_buffer.clear();
                }

                // Flush completed lines so start_idx is accurate
                for (styled_line, raw_line) in completed.drain(..) {
                    self.append_msg(styled_line, raw_line, RawMessageType::LLM);
                }

                self.stream.code_block = true;
                self.stream.code_block_buffer.clear();
                self.stream.code_block_lang = lang.clone();
                // Match `mermaid_fence_opener`: detect Mermaid from the first
                // whitespace-separated info token, case-insensitively, without
                // changing the stored language metadata for ordinary code.
                self.stream.code_block_is_mermaid = lang
                    .split_whitespace()
                    .next()
                    .is_some_and(|token| token.eq_ignore_ascii_case("mermaid"));
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
                        let (styled, raw) = format_table(
                            &self.stream.table_buffer,
                            &self.theme,
                            Some(self.log_scroll.width as usize),
                        );
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
                        let (styled, raw) = format_table(
                            &self.stream.table_buffer,
                            &self.theme,
                            Some(self.log_scroll.width as usize),
                        );
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
        self.scroll_log_to_bottom();
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

    /// Renders `/plugin list` as a titled table block (same style as `/skills`).
    fn show_plugin_list(&mut self, plugins: &[tact::plugin::InstalledPlugin]) {
        use crate::widgets::state::log_messages::classify_system_message;

        self.add_new_line();

        let msgs = self.msgs();
        let title = msgs
            .plugin_list_title_tmpl
            .replace("{}", &plugins.len().to_string());
        let title_ty = classify_system_message(&title);
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default().fg(self.theme.accent),
            )),
            title,
            title_ty,
        );
        self.add_new_line();

        if plugins.is_empty() {
            let empty = msgs.plugin_list_empty;
            self.append_msg(
                Line::from(Span::styled(empty, Style::default().fg(self.theme.fg))),
                empty.to_string(),
                classify_system_message(empty),
            );
        } else {
            let mut rows = vec![
                msgs.plugin_list_header.to_string(),
                "|---|---|---|".to_string(),
            ];
            rows.extend(plugins.iter().map(|plugin| {
                format!(
                    "| {} | {} | {} |",
                    plugin.id, plugin.marketplace, plugin.skill_count
                )
            }));
            let (styled, raw) =
                format_table(&rows, &self.theme, Some(self.log_scroll.width as usize));
            let ty = classify_system_message(&raw.first().cloned().unwrap_or_default());
            self.extend_msgs(styled, raw, ty);
        }

        self.add_new_line();

        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.scroll_log_to_bottom();
        }
    }

    /// Renders `/plugin marketplace list` as a titled table (one row per marketplace).
    ///
    /// Must not go through [`Self::add_system_message`]: a single-newline list would be
    /// Markdown-soft-broken into one crowded line.
    fn show_marketplace_list(&mut self, marketplaces: &[tact::plugin::MarketplaceRecord]) {
        use crate::widgets::state::log_messages::classify_system_message;

        self.add_new_line();

        let msgs = self.msgs();
        let title = msgs
            .marketplace_list_title_tmpl
            .replace("{}", &marketplaces.len().to_string());
        let title_ty = classify_system_message(&title);
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default().fg(self.theme.accent),
            )),
            title,
            title_ty,
        );
        self.add_new_line();

        if marketplaces.is_empty() {
            let empty = msgs.marketplace_list_empty;
            self.append_msg(
                Line::from(Span::styled(empty, Style::default().fg(self.theme.fg))),
                empty.to_string(),
                classify_system_message(empty),
            );
        } else {
            let mut rows = vec![
                msgs.marketplace_list_header.to_string(),
                "|---|---|".to_string(),
            ];
            rows.extend(marketplaces.iter().map(|marketplace| {
                format!(
                    "| {} | {} |",
                    marketplace.name,
                    marketplace.source.git_url()
                )
            }));
            let (styled, raw) =
                format_table(&rows, &self.theme, Some(self.log_scroll.width as usize));
            let ty = classify_system_message(&raw.first().cloned().unwrap_or_default());
            self.extend_msgs(styled, raw, ty);
        }

        self.add_new_line();

        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.scroll_log_to_bottom();
        }
    }

    /// Displays a completed plugin operation from the isolated worker.
    pub(crate) fn handle_plugin_event(&mut self, event: PluginEvent) {
        self.dirty = true;
        match event {
            PluginEvent::Succeeded {
                result,
                refresh_skills,
            } => {
                match &result {
                    PluginResult::ListedInstalled { plugins } => self.show_plugin_list(plugins),
                    PluginResult::ListedMarketplaces { marketplaces } => {
                        self.show_marketplace_list(marketplaces)
                    }
                    _ => self.add_system_message(format_plugin_result(&self.msgs(), &result)),
                }
                if refresh_skills && let Err(error) = crate::handlers::refresh_skills(self) {
                    self.add_system_message(
                        self.msgs().plugin_reload_failed_tmpl.replace("{}", &error),
                    );
                }
            }
            PluginEvent::Failed { operation, detail } => {
                let detail = sanitize_plugin_failure_detail(&detail);
                self.add_system_message(replace_two(
                    self.msgs().plugin_operation_failed_tmpl,
                    plugin_operation_label(&self.msgs(), &operation),
                    &detail,
                ));
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
    use std::{collections::HashMap, fs, path::PathBuf};

    use tact::plugin::{PluginEvent, PluginOperation, PluginResult};
    use tact_protocol::{
        AccountError, AccountUpdate, AgentErrorKind, AgentUpdate, PlanStep, StepResult, StepStatus,
        TaskSnapshot, TaskStatusSnapshot, TasksChangeReason, ThinkingChunk, ToolOutputChunk,
        ToolPresentationInfo,
    };
    use tokio::sync::mpsc::unbounded_channel;

    use super::MAX_PLUGIN_FAILURE_DETAIL_CHARS;
    use crate::{
        render::test_harness::render_log_panel_text,
        widgets::state::{App, Status},
    };

    fn make_app() -> App {
        let (_agent_tx, agent_rx) = unbounded_channel();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
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

    #[test]
    fn tasks_changed_shows_panel_without_touching_log() {
        let mut app = make_app();
        assert!(!app.task_panel.visible);
        let log_len_before = app.raw_messages.len();
        app.handle_agent_update(AgentUpdate::TasksChanged {
            tasks: vec![TaskSnapshot {
                id: 1,
                subject: "Fix auth".into(),
                status: TaskStatusSnapshot::InProgress,
                owner: String::new(),
                blocks: Vec::new(),
                blocked_by: Vec::new(),
                ..Default::default()
            }],
            reason: TasksChangeReason::Created,
        });
        assert!(app.task_panel.session_seen);
        assert!(app.task_panel.visible);
        assert!(
            app.task_panel.expanded,
            "sticky should default to expanded on first show"
        );
        assert_eq!(
            app.task_panel.snapshot.first().map(|t| t.subject.as_str()),
            Some("Fix auth"),
            "sticky snapshot should carry the subject"
        );
        assert_eq!(
            app.raw_messages.len(),
            log_len_before,
            "the task_* tool row already covers this in the Log, got:\n{:?}",
            app.raw_messages
        );
    }

    #[test]
    fn tasks_dag_popup_refreshes_when_new_tasks_arrive() {
        let mut app = make_app();
        // Baseline: one task, open the DAG popup.
        app.handle_agent_update(AgentUpdate::TasksChanged {
            tasks: vec![TaskSnapshot {
                id: 1,
                subject: "old".into(),
                status: TaskStatusSnapshot::Pending,
                owner: String::new(),
                blocks: Vec::new(),
                blocked_by: Vec::new(),
                ..Default::default()
            }],
            reason: TasksChangeReason::Created,
        });
        app.open_task_dag_popup();
        assert!(app.task_dag_popup.is_some());
        let before = app
            .task_dag_popup
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            before.contains("old"),
            "baseline should list old task:\n{before}"
        );
        assert!(!before.contains("new"), "new task not added yet:\n{before}");

        // A newer task is created while the popup is open.
        app.handle_agent_update(AgentUpdate::TasksChanged {
            tasks: vec![
                TaskSnapshot {
                    id: 1,
                    subject: "old".into(),
                    status: TaskStatusSnapshot::Pending,
                    owner: String::new(),
                    blocks: Vec::new(),
                    blocked_by: Vec::new(),
                    ..Default::default()
                },
                TaskSnapshot {
                    id: 2,
                    subject: "new".into(),
                    status: TaskStatusSnapshot::Pending,
                    owner: String::new(),
                    blocks: Vec::new(),
                    blocked_by: Vec::new(),
                    ..Default::default()
                },
            ],
            reason: TasksChangeReason::Created,
        });

        let after = app
            .task_dag_popup
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            after.contains("new"),
            "open DAG popup must refresh with newly added tasks, got:\n{after}"
        );
    }

    #[test]
    fn tasks_changed_hides_when_no_open_items() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::TasksChanged {
            tasks: vec![TaskSnapshot {
                id: 1,
                subject: "done".into(),
                status: TaskStatusSnapshot::Completed,
                owner: String::new(),
                blocks: Vec::new(),
                blocked_by: Vec::new(),
                ..Default::default()
            }],
            reason: TasksChangeReason::Updated,
        });
        assert!(app.task_panel.session_seen);
        assert!(!app.task_panel.visible);
        assert!(!app.task_panel.expanded);
    }

    fn write_skill(work_dir: &std::path::Path, name: &str) {
        let skill_dir = work_dir.join(".claude/skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n\n{name} body"),
        )
        .unwrap();
    }

    fn app_with_registry(work_dir: &std::path::Path) -> App {
        write_skill(work_dir, "existing");
        let mut app = make_app();
        app.work_dir = work_dir.to_path_buf();
        app.skill_registry = std::sync::Arc::new(std::sync::Mutex::new(
            tact::skill::get_skill_registry(work_dir).unwrap(),
        ));
        app
    }

    #[test]
    fn non_refreshing_plugin_success_preserves_the_registry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_registry(temp_dir.path());
        write_skill(temp_dir.path(), "new");

        app.handle_plugin_event(PluginEvent::Succeeded {
            result: PluginResult::ListedInstalled {
                plugins: Vec::new(),
            },
            refresh_skills: false,
        });

        let registry = tact::skill::lock_skills(&app.skill_registry);
        assert!(registry.skills().contains_key("existing"));
        assert!(!registry.skills().contains_key("new"));
    }

    #[test]
    fn failed_plugin_event_preserves_the_registry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_registry(temp_dir.path());
        write_skill(temp_dir.path(), "new");

        app.handle_plugin_event(PluginEvent::Failed {
            operation: PluginOperation::Install {
                plugin: "plugin".into(),
                marketplace: "fixture".into(),
            },
            detail: "technical detail".into(),
        });

        let registry = tact::skill::lock_skills(&app.skill_registry);
        assert!(registry.skills().contains_key("existing"));
        assert!(!registry.skills().contains_key("new"));
    }

    #[test]
    fn refreshing_plugin_success_reloads_the_registry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_registry(temp_dir.path());
        write_skill(temp_dir.path(), "new");

        app.handle_plugin_event(PluginEvent::Succeeded {
            result: PluginResult::Installed {
                plugin: "plugin".into(),
                marketplace: "fixture".into(),
            },
            refresh_skills: true,
        });

        let registry = tact::skill::lock_skills(&app.skill_registry);
        assert!(registry.skills().contains_key("existing"));
        assert!(registry.skills().contains_key("new"));
    }

    #[test]
    fn plugin_success_message_uses_chinese_templates() {
        let mut app = make_app();
        app.language = crate::i18n::Language::Chinese;

        app.handle_plugin_event(PluginEvent::Succeeded {
            result: PluginResult::Installed {
                plugin: "demo".into(),
                marketplace: "fixture".into(),
            },
            refresh_skills: false,
        });

        assert!(
            app.raw_messages
                .iter()
                .any(|message| message.contains("已安装插件 demo（来自 fixture）"))
        );
    }

    #[test]
    fn plugin_success_message_uses_english_templates() {
        let mut app = make_app();

        app.handle_plugin_event(PluginEvent::Succeeded {
            result: PluginResult::Installed {
                plugin: "demo".into(),
                marketplace: "fixture".into(),
            },
            refresh_skills: false,
        });

        assert!(
            app.raw_messages
                .iter()
                .any(|message| message.contains("Installed plugin demo from fixture"))
        );
    }

    #[test]
    fn plugin_list_renders_titled_table() {
        let mut app = make_app();

        app.handle_plugin_event(PluginEvent::Succeeded {
            result: PluginResult::ListedInstalled {
                plugins: vec![tact::plugin::InstalledPlugin {
                    id: "superpowers".into(),
                    marketplace: "superpowers-dev".into(),
                    revision: "abc123".into(),
                    cache_path: std::path::PathBuf::new(),
                    skill_count: 12,
                }],
            },
            refresh_skills: false,
        });

        let joined = app.raw_messages.join("\n");
        assert!(
            joined.contains("Installed plugins (1)"),
            "expected titled block, got:\n{joined}"
        );
        assert!(
            joined.contains("superpowers") && joined.contains("superpowers-dev"),
            "table should show plugin and marketplace, got:\n{joined}"
        );
        assert!(
            joined.contains("12"),
            "table should show skill count, got:\n{joined}"
        );
        assert!(
            !joined.contains("installed plugins:"),
            "old flat message must be gone, got:\n{joined}"
        );
    }

    #[test]
    fn marketplace_list_renders_titled_table_with_separate_rows() {
        let mut app = make_app();

        app.handle_plugin_event(PluginEvent::Succeeded {
            result: PluginResult::ListedMarketplaces {
                marketplaces: vec![
                    tact::plugin::MarketplaceRecord {
                        name: "claude-plugins-official".into(),
                        source: tact::plugin::MarketplaceSource::GitUrl(
                            "https://github.com/anthropics/claude-plugins-official.git".into(),
                        ),
                    },
                    tact::plugin::MarketplaceRecord {
                        name: "superpowers-dev".into(),
                        source: tact::plugin::MarketplaceSource::GitUrl(
                            "https://github.com/obra/superpowers.git".into(),
                        ),
                    },
                ],
            },
            refresh_skills: false,
        });

        let joined = app.raw_messages.join("\n");
        assert!(
            joined.contains("Marketplaces (2)"),
            "expected titled block, got:\n{joined}"
        );
        let official_line = app
            .raw_messages
            .iter()
            .find(|line| line.contains("claude-plugins-official"))
            .expect("official marketplace row");
        let superpowers_line = app
            .raw_messages
            .iter()
            .find(|line| line.contains("superpowers-dev"))
            .expect("superpowers marketplace row");
        assert_ne!(
            official_line, superpowers_line,
            "each marketplace must be its own row, got:\n{joined}"
        );
        assert!(
            !joined.contains("marketplaces:"),
            "old crowded one-liner must be gone, got:\n{joined}"
        );
    }

    #[test]
    fn plugin_failure_message_uses_chinese_template_with_detail() {
        let mut app = make_app();
        app.language = crate::i18n::Language::Chinese;

        app.handle_plugin_event(PluginEvent::Failed {
            operation: PluginOperation::Install {
                plugin: "demo".into(),
                marketplace: "fixture".into(),
            },
            detail: "network timeout".into(),
        });

        assert!(
            app.raw_messages
                .iter()
                .any(|message| message == "安装插件失败：network timeout")
        );
    }

    #[test]
    fn plugin_failure_message_sanitizes_and_bounds_technical_detail() {
        let mut app = make_app();
        let detail = format!(
            "\u{1b}[31mnetwork\nerror\r\n\u{7}{}",
            "雪".repeat(MAX_PLUGIN_FAILURE_DETAIL_CHARS + 1)
        );

        app.handle_plugin_event(PluginEvent::Failed {
            operation: PluginOperation::Install {
                plugin: "demo".into(),
                marketplace: "fixture".into(),
            },
            detail,
        });

        let message = app
            .raw_messages
            .iter()
            .find(|message| message.starts_with("install plugin failed: "))
            .unwrap();
        let sanitized_detail = message.strip_prefix("install plugin failed: ").unwrap();
        assert!(!sanitized_detail.chars().any(char::is_control));
        assert!(sanitized_detail.starts_with(" [31mnetwork error  "));
        assert!(sanitized_detail.ends_with("..."));
        assert_eq!(
            sanitized_detail.chars().count(),
            MAX_PLUGIN_FAILURE_DETAIL_CHARS + 3
        );
        assert!(sanitized_detail.contains('雪'));
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
            presentation: ToolPresentationInfo::generic("bash"),
        });
    }

    #[test]
    fn bash_live_output_grows_to_three_rows_then_keeps_a_three_line_tail() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        let initial_rows = app.tools.active[0].output.visual_rows(false);

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("one\n")],
        });
        let one_row = app.tools.active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("two\n")],
        });
        let two_rows = app.tools.active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("three\n")],
        });
        let three_rows = app.tools.active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("four\n")],
        });

        assert!(initial_rows < one_row && one_row < two_rows && two_rows < three_rows);
        assert_eq!(app.tools.active[0].output.visual_rows(false), three_rows);
        assert_eq!(app.tools.active[0].output.detail_preview.len(), 3);
        assert_eq!(
            app.tools.active[0]
                .output
                .detail_preview
                .iter()
                .map(|line| line.plain_text())
                .collect::<Vec<_>>(),
            ["two", "three", "four"]
        );
    }

    #[test]
    fn progress_does_not_repin_scrolled_log() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");
        app.log_scroll.visual_top = 3;

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("line\n")],
        });

        assert_eq!(app.log_scroll.visual_top, 3);
    }

    #[test]
    fn progress_keeps_bottom_pinned_log_on_live_output_growth() {
        let mut app = make_app();
        seed_running_bash(&mut app, "b1");

        let _ = render_log_panel_text(&mut app, 80, 4);
        assert_ne!(
            app.log_scroll.offset,
            u16::MAX,
            "render clamps the bottom sentinel"
        );

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("live line\n")],
        });

        assert_eq!(app.log_scroll.offset, u16::MAX);
        let rendered = render_log_panel_text(&mut app, 80, 4);
        assert!(
            rendered.contains("live line"),
            "bottom-pinned viewport should follow the live Bash output, got:\n{rendered}"
        );
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

    // ---- background_run keep-live card lifecycle ----

    fn background_presentation() -> ToolPresentationInfo {
        let mut presentation = ToolPresentationInfo::generic("background_run");
        presentation.keep_live = true;
        presentation
    }

    fn seed_running_background(app: &mut App, tool_id: &str) {
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "run build in background",
            "background_run",
            tool_id,
            HashMap::from([("command".to_string(), "cargo build".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: tool_id.to_string(),
            tool_name: "background_run".into(),
            arg_summary: "cargo build".into(),
            arg_full: "cargo build".into(),
            presentation: background_presentation(),
        });
    }

    #[test]
    fn background_step_finished_keeps_card_live() {
        let mut app = make_app();
        seed_running_background(&mut app, "bg1");

        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "bg1".into(),
            result: StepResult {
                tool: "background_run".into(),
                arg_summary: "cargo build".into(),
                arg_full: Some("cargo build".into()),
                status: StepStatus::Success,
                message: "Background task 018f3a2c started: cargo build".into(),
                detail: None,
                duration_us: Some(1200),
                permission_label: None,
                presentation: background_presentation(),
            },
        });

        assert_eq!(
            app.tools.active.len(),
            1,
            "background card must stay active after StepFinished"
        );
        assert!(app.tools.blocks.is_empty());
        // The plan step records the started message, not a final result.
        assert_eq!(
            app.plan.steps[0].output.as_deref(),
            Some("Background task 018f3a2c started: cargo build")
        );
    }

    #[test]
    fn background_task_finished_finalizes_success_card() {
        let mut app = make_app();
        seed_running_background(&mut app, "bg1");
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "bg1".into(),
            chunks: vec![ToolOutputChunk::stdout("Compiling ...\n")],
        });
        assert!(
            app.tools.active[0].output.visual_rows(false) > 2,
            "live progress should grow the active card"
        );

        app.handle_agent_update(AgentUpdate::BackgroundTaskFinished {
            tool_id: "bg1".into(),
            success: true,
            message: "Background task 018f3a2c completed".into(),
            output: "Compiling ...\ndone".into(),
        });

        assert!(app.tools.active.is_empty(), "card must be finalized");
        assert_eq!(app.tools.blocks.len(), 1);
        let block = &app.tools.blocks[0];
        assert_eq!(block.tool_id, "bg1");
        assert!(matches!(
            block.output.phase,
            crate::widgets::tool_widget::ToolPhase::Success
        ));
        assert!(
            block.output.duration_us.is_some(),
            "completed card should carry a duration"
        );
        let detail = block.output.detail_full.clone().unwrap_or_default();
        assert!(detail.contains("done"), "detail: {detail}");
    }

    #[test]
    fn background_task_finished_finalizes_failed_card() {
        let mut app = make_app();
        seed_running_background(&mut app, "bg1");

        app.handle_agent_update(AgentUpdate::BackgroundTaskFinished {
            tool_id: "bg1".into(),
            success: false,
            message: "Background task 018f3a2c failed".into(),
            output: "error: build failed".into(),
        });

        assert!(app.tools.active.is_empty(), "card must be finalized");
        let block = &app.tools.blocks[0];
        assert!(matches!(
            block.output.phase,
            crate::widgets::tool_widget::ToolPhase::Failed
        ));
        assert!(
            block
                .output
                .detail_full
                .as_deref()
                .unwrap_or_default()
                .contains("build failed"),
            "failed card should expose the output"
        );
    }

    #[test]
    fn background_task_finished_without_live_card_adds_system_message() {
        let mut app = make_app();

        app.handle_agent_update(AgentUpdate::BackgroundTaskFinished {
            tool_id: "gone".into(),
            success: true,
            message: "Background task 018f3a2c completed".into(),
            output: String::new(),
        });

        assert!(app.tools.active.is_empty());
        assert!(app.tools.blocks.is_empty());
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("Background task 018f3a2c completed")),
            "missing fallback message: {:?}",
            app.raw_messages
        );
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
            chunks: vec![ToolOutputChunk::stdout("one\ntwo\nthree\nfour\n")],
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
                presentation: ToolPresentationInfo::generic("bash"),
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
            Some("$ long-command\n\nlive line\n")
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
    fn task_complete_appends_task_stats_block() {
        use tact_protocol::{ModelCallParams, TokenUsageInfo};

        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::TokenUsage(TokenUsageInfo {
            prompt: 100,
            completion: 50,
            total: 150,
            prompt_cache_hit_tokens: 10,
            prompt_cache_miss_tokens: 90,
            reasoning_tokens: 5,
        }));
        app.handle_agent_update(AgentUpdate::ModelInfo(ModelCallParams {
            model: "mock-model".into(),
            max_tokens: 8192,
            thinking_budget: None,
            reasoning_effort: None,
            extra_body: None,
        }));
        // Frozen elapsed time: separator reuses it when no start time is set.
        app.last_prompt_elapsed_secs = Some(65);

        app.handle_agent_update(AgentUpdate::TaskComplete("All done.".into()));

        let joined = app.raw_messages.join("\n");
        assert!(
            joined.contains("📊 任务统计：⏱ 01:05"),
            "elapsed part missing: {joined}"
        );
        assert!(
            joined.contains("🧠 mock-model"),
            "model part missing: {joined}"
        );
        assert!(
            joined.contains("150 tokens (prompt 100 · completion 50 · cache 10 · reasoning 5)"),
            "token part missing: {joined}"
        );
    }

    #[test]
    fn task_stats_block_skips_empty_parts() {
        let mut app = make_app();
        app.last_prompt_elapsed_secs = Some(5);

        app.handle_agent_update(AgentUpdate::TaskComplete("All done.".into()));

        let joined = app.raw_messages.join("\n");
        let stats_line = joined
            .lines()
            .find(|l| l.contains("📊 任务统计："))
            .expect("stats block missing");
        assert_eq!(stats_line, "📊 任务统计：⏱ 00:05  [copy]");
    }

    #[test]
    fn copy_turn_ending_at_stats_copies_last_turn_only() {
        let mut app = make_app();
        app.add_user_message("first question".into());
        app.add_system_message("first answer".into());
        app.last_prompt_elapsed_secs = Some(1);
        app.add_task_end_separator();
        app.add_task_stats_block();

        app.add_user_message("second question".into());
        app.add_system_message("second answer".into());
        app.last_prompt_elapsed_secs = Some(2);
        app.add_task_end_separator();
        app.add_task_stats_block();

        let stats_idx = app
            .raw_messages
            .iter()
            .rposition(|l| l.contains("📊 任务统计："))
            .expect("stats");
        app.copy_turn_ending_at_stats(stats_idx);
        let copy_notice = app.raw_messages.last().expect("copy notice");
        assert!(copy_notice.contains("已复制") || copy_notice.contains("Copied"));
        assert!(!copy_notice.contains("second question"));

        // Prefer clipboard_buffer when system clipboard is unavailable; otherwise
        // just verify the extracted range would exclude the first turn.
        let start = app
            .raw_messages
            .iter()
            .position(|l| l.contains("📊 任务统计："))
            .expect("first stats")
            + 1;
        let mut expected_parts = Vec::new();
        for i in start..stats_idx {
            let line = app.raw_messages[i].as_str();
            if line.is_empty()
                || crate::render::cells::separator::is_task_end_separator(line)
                || crate::widgets::state::is_task_stats_line(line)
            {
                continue;
            }
            expected_parts.push(line);
        }
        let expected = expected_parts.join("\n");
        assert!(
            expected.contains("second question") && expected.contains("second answer"),
            "expected second turn in {expected}"
        );
        assert!(
            !expected.contains("first question") && !expected.contains("first answer"),
            "first turn leaked into {expected}"
        );
        if !app.clipboard_buffer.is_empty() {
            assert_eq!(app.clipboard_buffer, expected);
        }
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
            presentation: ToolPresentationInfo::generic("read_file"),
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
                presentation: ToolPresentationInfo::generic("read_file"),
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
            arg_summary: String::new(),
            error: "file not found".into(),
        });
        assert!(matches!(app.status, Status::Idle));
    }

    #[test]
    fn step_failed_keeps_arg_summary_in_failed_card_title() {
        let mut app = make_app();
        // Started without a query (action not yet populated), then failed with
        // the query carried on the failure: the failed card title must show it.
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "ws_1".into(),
            tool_name: "web_search".into(),
            arg_summary: String::new(),
            arg_full: String::new(),
            presentation: tact_protocol::ToolPresentationInfo::generic("web_search"),
        });
        app.handle_agent_update(AgentUpdate::StepFailed {
            idx: 0,
            tool_id: "ws_1".into(),
            arg_summary: "Rust async".into(),
            error: "web search failed (status: Failed, query: \"Rust async\")".into(),
        });

        let block = app.tools.blocks.last().expect("failed tool block");
        let title = &block.output.title_raw;
        assert!(
            title.contains("Rust async"),
            "failed card title must keep the query, got: {title}"
        );
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
            presentation: ToolPresentationInfo::generic("read_file"),
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
                presentation: ToolPresentationInfo::generic("read_file"),
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
