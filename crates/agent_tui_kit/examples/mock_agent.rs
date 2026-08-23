//! Headless mock-agent consumer (plan T5.1).
//!
//! Proves the kit stands alone: a mock agent emits a full `AgentUpdate`
//! sequence (thinking → step started → tool progress → stream chunk → task
//! complete), the example applies it to kit state and renders headless
//! frames (status bar, log panel, input box, bottom bar) with **no** `tact`
//! / `tact_llm` dependency — only `agent_tui_kit` + ratatui.
//!
//! Run: `cargo run -p agent_tui_kit --example mock_agent`

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::Borders,
};

use agent_tui_kit::{
    bridge::Command,
    i18n::{Language, Messages},
    protocol::{
        AgentUpdate, PlanStep, StepResult, StepStatus, ThinkingChunk, TokenUsageInfo,
        ToolPresentationInfo,
    },
    render::{
        bar::{render_bottom_bar, render_status_bar},
        ctx::RenderCtx,
        input::render_input_box,
        log::render_log_panel_pure,
        util::wrap_line,
    },
    state::{
        ActiveThinkingBlock, FocusedPanel, InputMode, LogCoordinator, LogItemKind, LogScroll,
        MouseState, PlanPanel, SelectPopup, Status, StatusBarState, StreamState, TaskPanelState,
        ThinkingBlock, ThinkingState, ToolState,
    },
    theme::{Theme, ThemeName},
};

/// Minimal host-side shell: kit state + a command outbox.
struct MockShell {
    theme: Theme,
    messages: Messages,
    log: LogCoordinator,
    log_scroll: LogScroll,
    stream: StreamState,
    thinking: ThinkingState,
    tools: ToolState,
    plan: PlanPanel,
    status: Status,
    status_bar: StatusBarState,
    input: String,
    input_cursor: usize,
    cmd_line: String,
    /// Commands the "agent" received (the kit's outbound contract).
    outbox: Vec<Command>,
}

impl MockShell {
    fn new() -> Self {
        let messages = Messages::by_language(Language::English);
        Self {
            theme: Theme::from(ThemeName::Ink),
            messages,
            log: LogCoordinator::default(),
            log_scroll: LogScroll::new(),
            stream: StreamState::default(),
            thinking: ThinkingState::default(),
            tools: ToolState::default(),
            plan: PlanPanel::default(),
            status: Status::Idle,
            status_bar: StatusBarState::new("main".into()),
            input: String::new(),
            input_cursor: 0,
            cmd_line: String::new(),
            outbox: Vec::new(),
        }
    }

    /// Simplified host-side dispatch for the kit's protocol types.
    fn on_update(&mut self, update: AgentUpdate) {
        match update {
            AgentUpdate::ThinkingChunk(chunk) => match chunk {
                ThinkingChunk::Started => {
                    self.log
                        .append_msg(Line::from(""), String::new(), LogItemKind::Thinking);
                    let phys = self.log.items.len().saturating_sub(1);
                    self.thinking.active = Some(ActiveThinkingBlock::new(phys, Instant::now()));
                }
                ThinkingChunk::Delta(delta) => {
                    if let Some(active) = &mut self.thinking.active {
                        active.push_delta(&delta);
                    }
                }
                ThinkingChunk::Finished => {
                    if let Some(active) = self.thinking.active.take()
                        && !active.is_blank()
                    {
                        self.thinking.blocks.push(ThinkingBlock {
                            phys_idx: active.phys_idx,
                            content: active.content.clone(),
                            summary: active.content.lines().next().unwrap_or("").to_string(),
                            cached_markdown: vec![Line::from(active.content.clone())],
                            elapsed: std::time::Duration::from_millis(120),
                        });
                    }
                }
            },
            AgentUpdate::StepAdded(step) => {
                self.plan.steps.push(step);
            }
            AgentUpdate::StepStarted {
                idx,
                tool_id,
                tool_name,
                arg_summary,
                presentation,
                ..
            } => {
                self.status = Status::Executing {
                    current_step: idx,
                    total: self.plan.steps.len(),
                };
                let output = agent_tui_kit::widgets::tool_widget::ToolWidget::new(
                    &self.theme,
                    &self.messages,
                )
                .with_tool(tool_name)
                .with_arg_summary(arg_summary)
                .with_presentation(presentation)
                .build();
                self.tools
                    .active
                    .push(agent_tui_kit::state::ActiveToolBlock {
                        phys_idx: self.log.items.len().saturating_sub(1),
                        tool_id,
                        output,
                        live_output: tact_protocol::tool_output::ToolOutputBuffer::new_full(1024),
                        started_at: Instant::now(),
                    });
            }
            AgentUpdate::StepFinished { tool_id, .. } => {
                self.tools.active.retain(|a| a.tool_id != tool_id);
            }
            AgentUpdate::StreamChunk(chunk) => {
                self.stream.buffer.push_str(&chunk);
            }
            AgentUpdate::TaskComplete(reply) => {
                self.status = Status::Done;
                // Flush any buffered stream text first (the real app splices
                // the stream into the reply row; here it is a separate row).
                if !self.stream.buffer.is_empty() {
                    self.stream.buffer.clear();
                }
                self.log.append_msg(
                    Line::from(reply.clone()),
                    reply,
                    LogItemKind::AssistantMarkdown,
                );
            }
            AgentUpdate::TokenUsage(usage) => {
                self.status_bar.token_total = usage.total;
                self.status_bar.token_prompt = usage.prompt;
                self.status_bar.token_completion = usage.completion;
            }
            _ => {}
        }
    }

    /// Minimal prepare: build the log-scroll caches the pure renderer reads.
    /// (The Tact app's real prepare additionally applies skill styling.)
    fn prepare_log_frame(&mut self, area: Rect) {
        let height = area.height.saturating_sub(2);
        self.log_scroll.height = height;
        let max_width = area.width.saturating_sub(2).max(1) as usize;

        // Phase 0: visible indices.
        let stale = self.log_scroll.visible_indices_ver != self.log.items.len();
        if stale {
            self.log_scroll.visible_indices.clear();
            self.log_scroll.phys_to_logical_cache.clear();
            self.log_scroll
                .phys_to_logical_cache
                .resize(self.log.items.len(), None);
            for phys in 0..self.log.items.len() {
                self.log_scroll.visible_indices.push(phys);
                self.log_scroll.phys_to_logical_cache[phys] = Some(phys);
            }
            self.log_scroll.visible_indices_ver = self.log.items.len();
        }

        // Phase 1: logical → visual wrap cache.
        let cache_valid = self.log_scroll.visual_cache_ver == self.log.items.len()
            && self.log_scroll.visual_cache_width == max_width as u16
            && self.log_scroll.visual_cache_theme == self.theme.name;
        if !cache_valid {
            self.log_scroll.visual_cache.clear();
            self.log_scroll.visual_start_cache.clear();
            self.log_scroll.visual_start_cache.push(0);
            for logical_i in 0..self.log_scroll.visible_indices.len() {
                let line = match self.log_scroll.visible_indices.get(logical_i) {
                    Some(&phys) => {
                        let item = &self.log.items[phys];
                        if item.line.spans.is_empty() {
                            Line::default()
                        } else {
                            item.line.clone()
                        }
                    }
                    None => Line::default(),
                };
                let wrapped = wrap_line(&line, max_width);
                self.log_scroll.visual_cache.extend(wrapped);
                self.log_scroll
                    .visual_start_cache
                    .push(self.log_scroll.visual_cache.len());
            }
            self.log_scroll.visual_cache_width = max_width as u16;
            self.log_scroll.visual_cache_ver = self.log.items.len();
            self.log_scroll.visual_cache_theme = self.theme.name;
        }

        // Phase 2: clamp to bottom.
        let total_visual = *self.log_scroll.visual_start_cache.last().unwrap_or(&0);
        let max_visual_scroll = total_visual.saturating_sub(self.log_scroll.height as usize);
        if self.log_scroll.visual_top == usize::MAX {
            self.log_scroll.visual_top = max_visual_scroll;
        } else {
            self.log_scroll.visual_top = self.log_scroll.visual_top.min(max_visual_scroll);
        }
    }

    /// Render one headless frame; returns the terminal text.
    fn render_frame(&mut self, width: u16, height: u16) -> String {
        self.prepare_log_frame(Rect::new(0, 1, width, height - 4));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                let size = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(1),
                        Constraint::Length(3),
                        Constraint::Length(2),
                    ])
                    .split(size);
                let ctx = RenderCtx {
                    theme: &self.theme,
                    messages: Messages::by_language(Language::English),
                    log_scroll: &self.log_scroll,
                    log: &self.log,
                    code_blocks: &[],
                    mermaid_blocks: &[],
                    tools: &self.tools,
                    thinking: &self.thinking,
                    stream: &self.stream,
                    mouse: &MouseState::default(),
                    skills_data: &[],
                    loading_idx: None,
                    spinner_frame: 3,
                    status_bar: &self.status_bar,
                    status: &self.status,
                    input_mode: InputMode::Normal,
                    focused_panel: FocusedPanel::Log,
                    language: Language::English,
                    workspace_dir: "/tmp/mock-ws",
                    model_context_window: 200_000,
                    process_start_time: &chrono::Local::now(),
                    task_start_time: None,
                    flash_msg: None,
                    account: None,
                    plan: &self.plan,
                    input: &self.input,
                    input_cursor: self.input_cursor,
                    input_scroll: 0,
                    cmd_line: &self.cmd_line,
                    pending_messages: &[],
                    input_voice_title: None,
                    code_popup: None,
                    mermaid_popup: None,
                    system_prompt_popup: None,
                    subagent_popup: None,
                    task_history: &[],
                    select: &SelectPopup::default(),
                    task_panel: &TaskPanelState::default(),
                };
                render_status_bar(frame, chunks[0], &ctx);
                render_log_panel_pure(frame, chunks[1], &ctx, Borders::ALL);
                let _cancel = render_input_box(frame, chunks[2], &ctx, &HashSet::new());
                render_bottom_bar(frame, chunks[3], &ctx);
            })
            .expect("draw");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}

/// The full mock sequence: thinking → step started → tool progress → stream
/// chunk → task complete (+ token usage), then a user command.
fn full_sequence() -> Vec<AgentUpdate> {
    vec![
        AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
        AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "Let me reason about this carefully.\nSecond reasoning line.\n".into(),
        )),
        AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
        AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "main.rs".to_string())]),
        )),
        AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "tool_read_1".into(),
            tool_name: "read_file".into(),
            arg_summary: "main.rs".into(),
            arg_full: "main.rs".into(),
            presentation: ToolPresentationInfo::generic("read_file"),
        },
        AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "tool_read_1".into(),
            result: StepResult {
                tool: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("fn main() {}".into()),
                duration_us: Some(1_000),
                permission_label: None,
                presentation: ToolPresentationInfo::generic("read_file"),
            },
        },
        AgentUpdate::StreamChunk("Hello from the mock agent. ".into()),
        AgentUpdate::StreamChunk("This text streams into the log.".into()),
        AgentUpdate::TokenUsage(TokenUsageInfo {
            prompt: 400,
            completion: 190,
            total: 590,
            prompt_cache_hit_tokens: 0,
            prompt_cache_miss_tokens: 0,
            reasoning_tokens: 0,
        }),
        AgentUpdate::TaskComplete(
            "Hello from the mock agent. This text streams into the log.".into(),
        ),
    ]
}

fn main() {
    let mut shell = MockShell::new();
    shell.status_bar.model_name = "mock-model".into();
    shell.status_bar.model_max_tokens = 128_000;

    for update in full_sequence() {
        shell.on_update(update);
    }

    // The host outbound contract: submit a task through the kit's Command type.
    shell
        .outbox
        .push(Command::SubmitTask("run the example".into()));
    assert_eq!(shell.outbox.len(), 1);

    let text = shell.render_frame(120, 30);
    assert!(
        text.contains("mock-model"),
        "status/bottom bar should show the model name:\n{text}"
    );
    // The simplified example prepare does not expand tall cells (thinking
    // cards / markdown) in the visual cache, so the reply row may sit outside
    // the viewport; the thinking card content proves the log pipeline works.
    assert!(
        text.contains("Let me reason"),
        "thinking card should render in the log:\n{text}"
    );
    assert!(
        text.contains("mock-ws"),
        "bottom bar should show the workspace path:\n{text}"
    );

    println!("headless mock agent OK — rendered frame:");
    println!("{text}");
}
