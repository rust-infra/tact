use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    widgets::ScrollbarState,
};
use tact_protocol::{
    AgentErrorKind, AgentUpdate, PlanStep, StepResult, TaskSnapshot, TasksChangeReason,
    ThinkingChunk, UserCommand,
};

use agent_tui_kit::{Ctx, PendingQueue, components::tool::ToolEvent, state::StreamEvent};

use crate::{
    render::render_md::{
        format_table_lines, render_markdown_tui, render_mermaid_block, render_plain_markdown,
    },
    widgets::state::*,
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
        self.coordinator_prepass(&update);
        // 1. Components update their own state (registry dispatch); the
        //    stream outbox carries parsed `StreamEvent`s the shell applies.
        let (stream_events, tool_events) = self.dispatch_components(&update);
        // 2. Apply the stream outbox to the shared log — StreamChunk only
        //    (the gap checks may append rows and must not run for other
        //    update types, mirroring the pre-dispatch `apply_stream_chunk`).
        if matches!(update, AgentUpdate::StreamChunk(_)) {
            self.apply_stream_events(stream_events);
        }
        // 3. Apply the tool-lifecycle outbox (placeholder rows, scroll).
        self.apply_tool_events(tool_events);
        // 4. Shell tail: rich behavior the kit components do not implement
        //    (log/status/scroll effects, select popups, plan writes).
        self.shell_handle(update);
        self.refresh_tail_scroll();
    }

    /// Route an update to the component registry (state-owner components).
    ///
    /// Returns the `StreamEvent` and `ToolEvent` outboxes the shell applies
    /// afterwards.
    ///
    /// `ThinkingChunk` is deliberately **not** dispatched: the shell owns the
    /// rich log-anchored thinking card (placeholder rows, scroll caches) —
    /// dispatching it would double-process. Every `Step*` update **is**
    /// dispatched now: `ToolComponent` owns the full tool-card lifecycle
    /// (active blocks, finalization) and emits `ToolEvent`s for the shell's
    /// log side effects.
    fn dispatch_components(&mut self, update: &AgentUpdate) -> (Vec<StreamEvent>, Vec<ToolEvent>) {
        if matches!(update, AgentUpdate::ThinkingChunk(_)) {
            return (Vec::new(), Vec::new());
        }
        // Field-split borrows: `Ctx` borrows the shell-owned shared surfaces
        // (log / input mode / pending queue) while the registry mutably
        // borrows the components.
        let Self {
            registry,
            log,
            input_mode,
            pending_messages,
            ..
        } = self;
        let mut stream_events: Vec<StreamEvent> = Vec::new();
        let mut tool_events: Vec<ToolEvent> = Vec::new();
        let mut pending = PendingQueue {
            items: std::mem::take(pending_messages),
        };
        let mut ctx = Ctx {
            log,
            input_mode: *input_mode,
            pending: &mut pending,
            stream_events: &mut stream_events,
            tool_events: &mut tool_events,
        };
        registry.dispatch_update(update, &mut ctx);
        *pending_messages = pending.items;
        (stream_events, tool_events)
    }

    /// Apply the `ToolEvent` outbox: the log side effects of the tool-card
    /// lifecycle (placeholder-row allocation/resize/removal, gap rows,
    /// scroll). The component already mutated its own `ToolState`.
    ///
    /// Ordering mirrors the pre-dispatch handlers: stream residue is flushed
    /// before placeholder rows are touched (`StepStarted`/`StepFinished` /
    /// `StepFailed` flushed first in the old code), and `Missing` messages
    /// are appended after the flush.
    fn apply_tool_events(&mut self, events: Vec<ToolEvent>) {
        if events.is_empty() {
            return;
        }
        // Step* semantics: flush leftover stream text before allocating tool
        // placeholder rows (Resize-only batches from ToolProgress never
        // flush, matching the old `on_tool_progress`).
        let needs_flush = events.iter().any(|e| {
            matches!(
                e,
                ToolEvent::Started { .. }
                    | ToolEvent::Cancelled { .. }
                    | ToolEvent::Finalized { .. }
            )
        });
        if needs_flush {
            self.flush_stream_pending();
        }
        for event in events {
            match event {
                ToolEvent::Started { tool_id, rows } => {
                    let phys_idx = self.push_tool_placeholder_rows(rows);
                    self.tools_mut().set_phys_idx(&tool_id, phys_idx);
                    self.refresh_tool_log_scroll();
                }
                ToolEvent::Resize {
                    phys_idx,
                    old_rows,
                    new_rows,
                    ..
                } => {
                    let was_pinned = self.is_log_pinned_to_bottom();
                    self.resize_tool_placeholder_rows(phys_idx, old_rows, new_rows);
                    self.log_scroll.state =
                        ScrollbarState::new(self.total_log_lines().saturating_sub(1));
                    if was_pinned {
                        self.scroll_log_to_bottom();
                    }
                }
                ToolEvent::Finalized {
                    tool_id,
                    old_rows,
                    new_rows,
                    had_active,
                } => {
                    // The component moved active → blocks; find the block's
                    // phys_idx. `had_active: false` means no active card
                    // existed (a finalize after a restart) — allocate the
                    // placeholder rows instead of resizing.
                    if let Some(block) = self
                        .tools()
                        .blocks
                        .iter()
                        .rev()
                        .find(|b| b.tool_id == tool_id)
                    {
                        if !had_active {
                            let phys_idx = self.push_tool_placeholder_rows(new_rows);
                            self.tools_mut().set_blocks_phys_idx(&tool_id, phys_idx);
                        } else {
                            self.resize_tool_placeholder_rows(block.phys_idx, old_rows, new_rows);
                        }
                        self.refresh_tool_log_scroll();
                    }
                }
                ToolEvent::Cancelled { phys_idx, rows, .. } => {
                    // The component removed the active card; drop its
                    // placeholder rows. A Started event always follows.
                    if phys_idx < self.log.items.len()
                        && phys_idx + rows <= self.log.items.len()
                        && rows > 0
                    {
                        self.drain_msgs(phys_idx..phys_idx + rows);
                        self.shift_phys_indices_from(phys_idx + rows, -(rows as isize));
                    }
                }
                ToolEvent::Missing { message } => {
                    self.add_system_message(message);
                }
            }
        }
    }

    /// Shell tail for updates the components do not (fully) handle: status
    /// and log effects, select popups, the rich thinking card, and plan-step
    /// output writes. Components claimed `TokenUsage` / `ModelInfo`
    /// (StatusBarComponent), `ToolProgress` / `ToolMeta` / `StepStarted` /
    /// `StepFinished` / `StepFailed` / `BackgroundTaskFinished` (ToolComponent,
    /// via the tool-event outbox applied in `apply_tool_events`), `StepAdded`
    /// (PlanComponent), `TasksChanged` (TaskPanelComponent), and `StreamChunk`
    /// (StreamComponent, via the stream outbox) during dispatch.
    fn shell_handle(&mut self, update: AgentUpdate) {
        match update {
            AgentUpdate::StepAdded(step) => self.on_step_added(step),
            AgentUpdate::StepStarted { idx, tool_id, .. } => {
                self.on_step_started_tail(idx, tool_id);
            }
            AgentUpdate::StepFinished {
                idx,
                tool_id,
                result,
            } => self.on_step_finished_tail(idx, tool_id, result),
            AgentUpdate::StepFailed { .. } => self.on_step_failed_tail(),
            AgentUpdate::TaskComplete(summary) => {
                // Task complete: flush leftover streaming lines
                self.flush_stream_pending();
                // Don't re-render summary into messages (StreamChunk already displayed it).
                // Summary is only saved to task_history for history viewing.
                if let Some(entry) = self.task_history.last_mut() {
                    entry.summary = summary;
                }
                // Trailing separator: bumps `log_items.len()` to rebuild the visual wrap
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
                // or queued (pending) messages would be flushed against a stale
                // busy state.
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
            // Add system message
            AgentUpdate::Info(msg) => {
                self.add_system_message(msg);
            }
            // Whole-Markdown notice, rendered as a single MarkdownCell
            AgentUpdate::MdInfo(msg) => {
                self.append_system_markdown(msg);
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
                request_id,
                log_confirm,
            } => {
                self.select_kind = SelectKind::Agent;
                if self.select.request_id.is_some() {
                    // A prior agent select is still open — queue this one so a
                    // concurrent subagent's permission prompt doesn't overwrite
                    // (and hang) the first waiter.
                    self.pending_agent_selects.push_back(AgentSelectRequest {
                        prompt,
                        options,
                        request_id,
                        multi: false,
                        log_confirm,
                    });
                } else {
                    self.select.set(prompt, options, request_id, log_confirm);
                    self.input_mode = InputMode::Select;
                }
            }
            AgentUpdate::RequestMultiSelect {
                prompt,
                options,
                request_id,
            } => {
                self.select_kind = SelectKind::Agent;
                // Choice is shown on the ask_user tool meta row; no duplicate log line.
                if self.select.request_id.is_some() {
                    self.pending_agent_selects.push_back(AgentSelectRequest {
                        prompt,
                        options,
                        request_id,
                        multi: true,
                        log_confirm: false,
                    });
                } else {
                    self.select.set_multi(prompt, options, request_id, false);
                    self.input_mode = InputMode::Select;
                }
            }
            AgentUpdate::ThinkingChunk(chunk) => {
                match chunk {
                    ThinkingChunk::Started => {
                        self.begin_thinking_block();
                    }
                    ThinkingChunk::Delta(text) => {
                        // Started may be missing on older producers — open on first delta.
                        if self.thinking_mut().active.is_none() {
                            self.begin_thinking_block();
                        }
                        self.append_thinking_delta(&text);
                    }
                    ThinkingChunk::Finished => {
                        self.flush_and_close_thinking();
                    }
                }
            }
            AgentUpdate::BackgroundTaskFinished {
                tool_id, message, ..
            } => self.on_background_task_finished_tail(&tool_id, &message),
            AgentUpdate::SubagentFinished {
                tool_id,
                child_id,
                success,
                summary,
            } => self.on_subagent_finished_tail(&tool_id, &child_id, success, &summary),
            // ToolProgress → ToolComponent (live output + Resize event).
            AgentUpdate::ToolProgress { .. } => {}
            AgentUpdate::TasksChanged { tasks, reason } => {
                self.on_tasks_changed_tail(tasks, reason);
            }
            // TokenUsage / ModelInfo → StatusBarComponent (dispatch).
            // ToolMeta → ToolComponent (dispatch).
            // StreamChunk → StreamComponent parse + apply_stream_events.
            AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolMeta { .. }
            | AgentUpdate::StreamChunk(_) => {}
        }
    }

    /// Coordinator pre-pass: reconcile cross-component invariants before any
    /// component handler sees the update.
    ///
    /// - Close an open thinking region on content-producing updates that are
    ///   not `ThinkingChunk` (explicit `Finished` is preferred; TokenUsage /
    ///   ModelInfo / ToolMeta must not close the region — they can arrive
    ///   mid-stream).
    /// - Remove the loading placeholder on any content-producing update
    ///   (metadata-only updates keep it).
    fn coordinator_prepass(&mut self, update: &AgentUpdate) {
        match update {
            AgentUpdate::ThinkingChunk(_)
            | AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolMeta { .. }
            | AgentUpdate::ToolProgress { .. } => {}
            _ => self.flush_and_close_thinking(),
        }
        match update {
            AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::ToolMeta { .. }
            | AgentUpdate::ToolProgress { .. } => {}
            _ => {
                self.remove_loading_placeholder();
            }
        }
    }

    /// Unified tail scroll refresh, covering helpers that inserted messages
    /// without updating scroll (e.g. flush_and_close_thinking /
    /// flush_stream_pending); redundant per-arm updates are cheap and harmless.
    fn refresh_tail_scroll(&mut self) {
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
    }

    /// Snapshot changes only drive the sticky panel; the Log already shows the
    /// originating `task_*` tool row, so no extra system message is appended.
    fn on_tasks_changed_tail(&mut self, _tasks: Vec<TaskSnapshot>, _: TasksChangeReason) {
        // TaskPanelComponent (registry dispatch) already applied the snapshot
        // (visibility/expand logic lives in the kit's `apply_snapshot`).
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
                render_task_dag_lines(&self.task_panel().snapshot, &self.theme, width);
            if let Some(p) = self.task_dag_popup.as_mut() {
                p.lines = lines;
                p.mermaid_source = source;
            }
        }
    }

    fn on_step_added(&mut self, _step: PlanStep) {
        // Flush leftover streaming text, preventing LLM output from appearing
        // between StepAdded and StepStarted.
        self.flush_stream_pending();
        // PlanComponent (registry dispatch) already recorded the step in
        // `plan.steps` / `plan.steps_set` before this tail ran.
        let idx = self.plan().steps.len();
        // Don't change current_step or total — the step hasn't started yet.
        // Ensure there is an Executing status before StepStarted arrives.
        self.ensure_executing_status(idx);
    }

    fn on_step_started_tail(&mut self, idx: usize, tool_id: String) {
        // ToolComponent (registry dispatch + apply_tool_events) already
        // created the active card and allocated its placeholder rows; the
        // tail only keeps status progress in sync.
        let idx = resolve_step_idx(&self.plan_mut().steps, &tool_id, idx);
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
    }

    fn on_step_finished_tail(&mut self, idx: usize, tool_id: String, result: StepResult) {
        let idx = resolve_step_idx(&self.plan_mut().steps, &tool_id, idx);
        // Keep-live tools (e.g. `background_run`) return immediately but their
        // card keeps streaming: the component skipped finalization; the plan
        // step is still recorded as done (the invocation did succeed at
        // "started").
        if result.presentation.keep_live {
            if let Some(step) = self.plan_mut().steps.get_mut(idx) {
                step.output = Some(result.message);
            }
            return;
        }
        if let Some(step) = self.plan_mut().steps.get_mut(idx) {
            step.output = Some(result.message);
        }
    }

    /// Plan-step write for a keep-live card closed by the background worker;
    /// the card finalization itself happened in the component (Finalized /
    /// Missing tool event, applied by `apply_tool_events`).
    fn on_background_task_finished_tail(&mut self, tool_id: &str, message: &str) {
        let step_idx = resolve_step_idx(&self.plan_mut().steps, tool_id, 0);
        if let Some(step) = self.plan_mut().steps.get_mut(step_idx) {
            step.output = Some(message.to_string());
        }
    }

    /// Plan-step write for a keep-live subagent card closed by the detached
    /// child, plus a wake-up relay to the driver so an idle parent resumes to
    /// consume the re-injected result (the parent `ui_tx` has no driver path).
    fn on_subagent_finished_tail(
        &mut self,
        tool_id: &str,
        child_id: &str,
        success: bool,
        summary: &str,
    ) {
        let step_idx = resolve_step_idx(&self.plan_mut().steps, tool_id, 0);
        if let Some(step) = self.plan_mut().steps.get_mut(step_idx) {
            step.output = Some(summary.to_string());
        }
        let _ = self
            .user_cmd_tx
            .send(UserCommand::SubagentFinishedNotification {
                child_id: child_id.to_string(),
                summary: summary.to_string(),
                success,
            });
    }

    fn on_step_failed_tail(&mut self) {
        // The card finalization (or Missing message) happened in the
        // component + apply_tool_events; the tail only keeps status in sync.
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
        is_mermaid: bool,
    ) {
        self.stream_mut().code_block_is_mermaid = false;

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
                    self.splice_msgs(
                        start..stream_end,
                        diagram,
                        raw,
                        LogItemKind::AssistantMarkdown,
                    );
                    start
                }
                None => {
                    let start = self.log.items.len();
                    self.extend_msgs(diagram, raw, LogItemKind::AssistantMarkdown);
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
                    LogItemKind::AssistantMarkdown,
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
                self.extend_msgs(styled, raw, LogItemKind::AssistantMarkdown);
            }
        }
    }

    fn apply_stream_events(&mut self, events: Vec<StreamEvent>) {
        // The gap checks run on every StreamChunk (even event-less ones — the
        // in-flight buffer line still needs the category gap), mirroring the
        // pre-dispatch behavior of `apply_stream_chunk`.
        self.ensure_gap_after_user_message();
        self.ensure_gap_after_tools();
        // Thinking region is closed by the safety gate above when still open.

        // StreamComponent (registry dispatch) already ran the parse state
        // machine (fence detection, table/paragraph/code buffering); this
        // shell loop applies the events to the log with app-layer rendering
        // (markdown, tables, indicators).
        for event in events {
            match event {
                StreamEvent::MarkdownParagraph { text } => {
                    let (styled, raw) = render_markdown_tui(&text, &self.theme);
                    for (styled_line, raw_line) in styled.into_iter().zip(raw) {
                        self.append_msg(styled_line, raw_line, LogItemKind::AssistantMarkdown);
                    }
                }
                StreamEvent::Table { rows } => {
                    let (styled, raw) =
                        format_table_lines(&rows, &self.theme, Some(self.table_layout_width()));
                    for (styled_line, raw_line) in styled.into_iter().zip(raw) {
                        self.append_msg(styled_line, raw_line, LogItemKind::AssistantMarkdown);
                    }
                }
                StreamEvent::Blank => {
                    self.append_msg(
                        Line::from(""),
                        String::new(),
                        LogItemKind::AssistantMarkdown,
                    );
                }
                StreamEvent::OpenCodeBlock { lang, is_mermaid } => {
                    // Container header: ╭─ lang ─────
                    let label = if lang.is_empty() {
                        "code".to_string()
                    } else {
                        lang.clone()
                    };
                    let header_text = format!("╭─ {} ", label);
                    self.stream_mut().code_block_start_idx = Some(self.log.items.len());
                    self.append_msg(
                        Line::from(Span::styled(
                            header_text.clone(),
                            Style::default().fg(Color::DarkGray).bg(CODE_BG),
                        )),
                        format!("```{}", lang),
                        LogItemKind::AssistantMarkdown,
                    );
                    let _ = is_mermaid; // carried on CloseCodeBlock for finalize
                }
                StreamEvent::CodeLine { text: line } => {
                    let prev_idx = self.log.items.len().saturating_sub(1);
                    if self.stream_mut().code_block_line_count > 1
                        && let Some(prev_item) = self.log.items.get_mut(prev_idx)
                        && prev_item.raw.ends_with(STREAMING_INDICATOR)
                    {
                        let clean = prev_item
                            .raw
                            .trim_end_matches(STREAMING_INDICATOR)
                            .to_string();
                        prev_item.raw = clean.clone();
                        prev_item.line = Line::from(vec![
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
                        LogItemKind::AssistantMarkdown,
                    );
                }
                StreamEvent::CloseCodeBlock {
                    lang,
                    lines,
                    line_count,
                    is_mermaid,
                } => {
                    let start_idx = self.stream_mut().code_block_start_idx.take();
                    let stream_end = start_idx.map(|s| s + line_count).unwrap_or(0);
                    self.finish_stream_code_block(
                        lang, lines, start_idx, stream_end, true, is_mermaid,
                    );
                }
            }
        }

        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        // Auto-scroll to bottom (u16::MAX clipped by render_log_panel to visual line count)
        self.scroll_log_to_bottom();
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

    use crate::widgets::state::app::extensions::MAX_PLUGIN_FAILURE_DETAIL_CHARS;
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
        assert!(!app.task_panel_mut().visible);
        let log_len_before = app.log.items.len();
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
        assert!(app.task_panel_mut().session_seen);
        assert!(app.task_panel_mut().visible);
        assert!(
            app.task_panel_mut().expanded,
            "sticky should default to expanded on first show"
        );
        assert_eq!(
            app.task_panel_mut()
                .snapshot
                .first()
                .map(|t| t.subject.as_str()),
            Some("Fix auth"),
            "sticky snapshot should carry the subject"
        );
        assert_eq!(
            app.log.items.len(),
            log_len_before,
            "the task_* tool row already covers this in the Log, got:\n{:?}",
            app.log.items
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
        assert!(app.task_panel_mut().session_seen);
        assert!(!app.task_panel_mut().visible);
        assert!(!app.task_panel_mut().expanded);
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
            app.log
                .items
                .iter()
                .any(|message| message.raw.contains("已安装插件 demo（来自 fixture）"))
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
            app.log
                .items
                .iter()
                .any(|message| message.raw.contains("Installed plugin demo from fixture"))
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
                    command_count: 0,
                    agent_count: 0,
                    has_hooks: false,
                    has_mcp: false,
                }],
            },
            refresh_skills: false,
        });

        let joined = app
            .log
            .items
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
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

        let joined = app
            .log
            .items
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Marketplaces (2)"),
            "expected titled block, got:\n{joined}"
        );
        let official_line = app
            .log
            .items
            .iter()
            .find(|item| item.raw.contains("claude-plugins-official"))
            .map(|item| item.raw.as_str())
            .expect("official marketplace row");
        let superpowers_line = app
            .log
            .items
            .iter()
            .find(|item| item.raw.contains("superpowers-dev"))
            .map(|item| item.raw.as_str())
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
            app.log
                .items
                .iter()
                .any(|message| message.raw == "安装插件失败：network timeout")
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
            .log
            .items
            .iter()
            .find(|item| item.raw.starts_with("install plugin failed: "))
            .map(|item| item.raw.as_str())
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
        let initial_rows = app.tools_mut().active[0].output.visual_rows(false);

        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("one\n")],
        });
        let one_row = app.tools_mut().active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("two\n")],
        });
        let two_rows = app.tools_mut().active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("three\n")],
        });
        let three_rows = app.tools_mut().active[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("four\n")],
        });

        assert!(initial_rows < one_row && one_row < two_rows && two_rows < three_rows);
        assert_eq!(
            app.tools_mut().active[0].output.visual_rows(false),
            three_rows
        );
        assert_eq!(app.tools_mut().active[0].output.detail_preview.len(), 3);
        assert_eq!(
            app.tools_mut().active[0]
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

        assert!(app.thinking_mut().active.is_some());
        assert_eq!(app.tools_mut().active[0].output.visual_rows(false), 2);
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
            app.tools_mut().active.len(),
            1,
            "background card must stay active after StepFinished"
        );
        assert!(app.tools_mut().blocks.is_empty());
        // The plan step records the started message, not a final result.
        assert_eq!(
            app.plan_mut().steps[0].output.as_deref(),
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
            app.tools_mut().active[0].output.visual_rows(false) > 2,
            "live progress should grow the active card"
        );

        app.handle_agent_update(AgentUpdate::BackgroundTaskFinished {
            tool_id: "bg1".into(),
            success: true,
            message: "Background task 018f3a2c completed".into(),
            output: "Compiling ...\ndone".into(),
        });

        assert!(app.tools_mut().active.is_empty(), "card must be finalized");
        assert_eq!(app.tools_mut().blocks.len(), 1);
        let block = &app.tools_mut().blocks[0];
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

        assert!(app.tools_mut().active.is_empty(), "card must be finalized");
        let block = &app.tools_mut().blocks[0];
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

        assert!(app.tools_mut().active.is_empty());
        assert!(app.tools_mut().blocks.is_empty());
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Background task 018f3a2c completed")),
            "missing fallback message: {:?}",
            app.log.items
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
        let phys_idx = app.tools_mut().active[0].phys_idx;

        app.open_diff_popup(phys_idx);

        let content = app
            .tools_mut()
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
        let live_rows = app.tools_mut().active[0].output.visual_rows(false);

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
        let completed_rows = app.tools_mut().blocks[0].output.visual_rows(false);
        app.handle_agent_update(AgentUpdate::ToolProgress {
            tool_id: "b1".into(),
            chunks: vec![ToolOutputChunk::stdout("late\n")],
        });

        assert!(completed_rows < live_rows);
        assert!(app.tools_mut().active.is_empty());
        assert_eq!(
            app.tools_mut().blocks[0].output.detail_full.as_deref(),
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

        let joined = app
            .log
            .items
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Task stats:⏱ 01:05"),
            "elapsed part missing: {joined}"
        );
        assert!(
            joined.contains("mock-model"),
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

        let joined = app
            .log
            .items
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let stats_line = joined
            .lines()
            .find(|l| l.contains("Task stats:"))
            .expect("stats block missing");
        assert_eq!(stats_line, "[copy]  Task stats:⏱ 00:05");
    }

    #[test]
    fn task_stats_block_localizes_prefix_and_copy_button() {
        let mut app = make_app();
        app.language = crate::i18n::Language::Chinese;
        app.last_prompt_elapsed_secs = Some(5);

        app.handle_agent_update(AgentUpdate::TaskComplete("All done.".into()));

        let joined = app
            .log
            .items
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let stats_line = joined
            .lines()
            .find(|l| l.contains("任务统计："))
            .expect("stats block missing");
        assert_eq!(stats_line, "[复制]  任务统计：⏱ 00:05");
    }

    #[test]
    fn task_stats_line_detection_covers_all_languages_and_legacy_rows() {
        use crate::widgets::state::is_task_stats_line;

        assert!(is_task_stats_line("[copy]  Task stats:⏱ 01:05"));
        assert!(is_task_stats_line("[复制]  任务统计：⏱ 01:05"));
        // Rows persisted before the icon was removed still need `[copy]` support
        // (legacy rows keep the button at the end).
        assert!(is_task_stats_line("📊 任务统计：⏱ 01:05  [copy]"));
        assert!(!is_task_stats_line("plain answer text"));
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
            .log
            .items
            .iter()
            .rposition(|item| item.raw.contains("Task stats:"))
            .expect("stats");
        app.copy_turn_ending_at_stats(stats_idx);
        let copy_notice = app.log.items.last().expect("copy notice");
        assert!(copy_notice.raw.contains("已复制") || copy_notice.raw.contains("Copied"));
        assert!(!copy_notice.raw.contains("second question"));

        // Prefer clipboard_buffer when system clipboard is unavailable; otherwise
        // just verify the extracted range would exclude the first turn.
        let start = app
            .log
            .items
            .iter()
            .position(|item| item.raw.contains("Task stats:"))
            .expect("first stats")
            + 1;
        let mut expected_parts = Vec::new();
        for i in start..stats_idx {
            let line = app.log.items[i].raw.as_str();
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

        assert_eq!(app.plan_mut().steps[0].output.as_deref(), Some("ok"));
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

        let block = app.tools_mut().blocks.last().expect("failed tool block");
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
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("LLM unavailable")),
            "error message should appear in log: {:?}",
            app.log.items
        );
    }

    #[test]
    fn info_update_appends_system_message() {
        let mut app = make_app();
        let before = app.log.items.len();
        app.handle_agent_update(AgentUpdate::Info("Cancelling...".into()));
        assert!(app.log.items.len() > before);
        assert!(
            app.log
                .items
                .last()
                .is_some_and(|item| item.raw.contains("Cancelling"))
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
        assert_eq!(app.status_bar_mut().token_prompt, 100);
        assert_eq!(app.status_bar_mut().token_completion, 50);
        assert_eq!(app.status_bar_mut().token_total, 150);
        assert_eq!(app.status_bar_mut().token_reasoning, 5);
    }

    #[test]
    fn request_select_enters_select_mode() {
        use crate::widgets::state::InputMode;

        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::RequestSelect {
            request_id: 1,
            prompt: "Allow bash?".into(),
            options: vec!["Yes".into(), "No".into()],
            log_confirm: false,
        });
        assert!(matches!(app.input_mode, InputMode::Select));
        assert!(app.select.prompt.contains("Allow bash"));
        assert_eq!(app.select.request_id, Some(1));
    }

    #[test]
    fn thinking_chunk_flushes_on_stream() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "reasoning line".into(),
        )));
        assert!(app.thinking_mut().active.is_some());
        app.handle_agent_update(AgentUpdate::StreamChunk("final answer".into()));
        assert!(app.thinking_mut().active.is_none());
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
        assert_eq!(app.status_bar_mut().model_name, "mock-model");
        assert_eq!(app.status_bar_mut().model_max_tokens, 4096);
        assert_eq!(app.status_bar_mut().model_thinking_budget, Some(32_000));
        assert_eq!(
            app.status_bar_mut().model_reasoning_effort.as_deref(),
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
        assert_eq!(app.plan_mut().steps.len(), 2);
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
            app.thinking_mut()
                .active
                .as_ref()
                .unwrap()
                .content
                .contains("part1")
        );
        assert!(
            app.thinking_mut()
                .active
                .as_ref()
                .unwrap()
                .content
                .contains("part2")
        );
        app.handle_agent_update(AgentUpdate::Info("done thinking".into()));
        assert!(app.thinking_mut().active.is_none());
    }

    #[test]
    fn thinking_finished_closes_without_other_update() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "done thinking\n".into(),
        )));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        assert!(app.thinking_mut().active.is_none());
        assert!(!app.thinking_mut().blocks.is_empty());
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
        assert!(app.thinking_mut().active.is_some());
        assert!(
            app.thinking_mut()
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
        let before = app.log.items.len();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        assert!(app.thinking_mut().active.is_some());
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        assert!(app.thinking_mut().active.is_none());
        assert!(app.thinking_mut().blocks.is_empty());
        assert_eq!(app.log.items.len(), before);
    }

    #[test]
    fn whitespace_only_delta_finished_leaves_no_thinking_block() {
        let mut app = make_app();
        let before = app.log.items.len();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "   ".into(),
        )));
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        assert!(app.thinking_mut().blocks.is_empty());
        assert!(app.thinking_mut().active.is_none());
        assert_eq!(app.log.items.len(), before);
    }

    #[test]
    fn thinking_finished_keeps_the_existing_placeholder_index() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "done thinking\n".into(),
        )));
        let phys_idx = app.thinking_mut().active.as_ref().unwrap().phys_idx;

        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));

        assert_eq!(app.thinking_mut().blocks[0].phys_idx, phys_idx);
        assert!(app.thinking_mut().active.is_none());
    }

    #[test]
    fn missing_thinking_started_creates_one_placeholder_not_source_rows() {
        let mut app = make_app();
        let before = app.log.items.len();

        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "first\nsecond".into(),
        )));

        assert_eq!(
            app.log.items.len(),
            before + crate::render::cells::thinking::thinking_visual_rows(2)
        );
        assert_eq!(
            app.thinking_mut()
                .active
                .as_ref()
                .unwrap()
                .display_tail()
                .len(),
            2
        );
    }
}
