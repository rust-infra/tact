//! Subagent sticky-pane state (mini log under the unified sticky host).

use tact_protocol::{AgentUpdate, ThinkingChunk};

const MAX_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StickyTab {
    #[default]
    Tasks,
    Subagent,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SubagentPaneState {
    pub(crate) parent_tool_id: String,
    pub(crate) session_id: String,
    pub(crate) lines: Vec<String>,
    pub(crate) stream_buf: String,
    pub(crate) thinking_buf: String,
    pub(crate) running: bool,
    pub(crate) scroll: usize,
    pub(crate) last_token_total: u32,
    /// Content retained after a run finishes (keeps sticky visible).
    pub(crate) has_content: bool,
}

impl SubagentPaneState {
    pub(crate) fn begin_run(&mut self, parent_tool_id: String, session_id: String) {
        self.parent_tool_id = parent_tool_id;
        self.session_id = session_id;
        self.lines.clear();
        self.stream_buf.clear();
        self.thinking_buf.clear();
        self.scroll = 0;
        self.running = true;
        self.has_content = true;
        self.last_token_total = 0;
    }

    pub(crate) fn mark_idle(&mut self) {
        self.flush_stream();
        self.flush_thinking();
        self.running = false;
    }

    pub(crate) fn visible_lines(&self, max_visible: usize) -> Vec<&str> {
        let start = self.scroll.min(self.lines.len());
        self.lines[start..]
            .iter()
            .take(max_visible)
            .map(String::as_str)
            .collect()
    }

    pub(crate) fn apply_update(&mut self, update: AgentUpdate) {
        self.has_content = true;
        self.running = true;
        match update {
            AgentUpdate::StreamChunk(text) => {
                self.stream_buf.push_str(&text);
                while let Some(idx) = self.stream_buf.find('\n') {
                    let line = self.stream_buf[..idx].to_string();
                    self.stream_buf = self.stream_buf[idx + 1..].to_string();
                    if !line.is_empty() {
                        self.push_line(format!("│ {line}"));
                    }
                }
            }
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started) => {
                self.thinking_buf.clear();
                self.push_line("… thinking".into());
            }
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(text)) => {
                self.thinking_buf.push_str(&text);
            }
            AgentUpdate::ThinkingChunk(ThinkingChunk::Finished) => {
                self.flush_thinking();
            }
            AgentUpdate::StepStarted {
                tool_name,
                arg_summary,
                ..
            } => {
                self.flush_stream();
                let summary = truncate(&arg_summary, 60);
                self.push_line(format!("→ {tool_name} {summary}"));
            }
            AgentUpdate::StepFinished { result, .. } => {
                let preview = truncate(&result.message, 50);
                self.push_line(format!("✓ {preview}"));
            }
            AgentUpdate::StepFailed { error, .. } => {
                self.push_line(format!("✗ {error}"));
            }
            AgentUpdate::ToolProgress { chunks, .. } => {
                for chunk in chunks {
                    if !chunk.text.is_empty() {
                        for line in chunk.text.lines() {
                            self.push_line(format!("  {line}"));
                        }
                    }
                }
            }
            AgentUpdate::Info(msg) => {
                self.push_line(format!("· {msg}"));
            }
            AgentUpdate::TokenUsage(usage) => {
                self.last_token_total = usage.total;
            }
            AgentUpdate::ModelInfo(params) => {
                self.push_line(format!("model {}", params.model));
            }
            AgentUpdate::Error(err) => {
                self.push_line(format!("error: {err:?}"));
                self.running = false;
            }
            // Lifecycle / parent-only — ignore if nested somehow.
            AgentUpdate::TaskComplete(_)
            | AgentUpdate::TaskCancelled
            | AgentUpdate::StepAdded(_)
            | AgentUpdate::SessionStats(_)
            | AgentUpdate::RequestSelect { .. }
            | AgentUpdate::RequestMultiSelect { .. }
            | AgentUpdate::TasksChanged { .. }
            | AgentUpdate::Subagent { .. } => {}
        }
    }

    fn flush_stream(&mut self) {
        if !self.stream_buf.is_empty() {
            let line = std::mem::take(&mut self.stream_buf);
            self.push_line(format!("│ {line}"));
        }
    }

    fn flush_thinking(&mut self) {
        if self.thinking_buf.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.thinking_buf);
        let preview = truncate(text.trim(), 80);
        if !preview.is_empty() {
            self.push_line(format!("… {preview}"));
        }
    }

    fn push_line(&mut self, line: String) {
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            let excess = self.lines.len() - MAX_LINES;
            self.lines.drain(0..excess);
            self.scroll = self.scroll.saturating_sub(excess);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{StepResult, StepStatus};

    #[test]
    fn stream_and_steps_become_lines() {
        let mut pane = SubagentPaneState::default();
        pane.begin_run("task-1".into(), "sess".into());
        pane.apply_update(AgentUpdate::StreamChunk("hello\nworld".into()));
        pane.apply_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "b1".into(),
            tool_name: "bash".into(),
            arg_summary: "ls".into(),
            arg_full: "ls".into(),
        });
        pane.apply_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "b1".into(),
            result: StepResult {
                tool: "bash".into(),
                arg_summary: "ls".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: None,
                duration_us: None,
                permission_label: None,
            },
        });
        assert!(pane.lines.iter().any(|l| l.contains("hello")));
        assert!(pane.lines.iter().any(|l| l.contains("bash")));
        assert!(pane.has_content);
    }

    #[test]
    fn token_usage_does_not_add_line() {
        let mut pane = SubagentPaneState::default();
        pane.begin_run("t".into(), "s".into());
        let before = pane.lines.len();
        pane.apply_update(AgentUpdate::TokenUsage(tact_protocol::TokenUsageInfo {
            total: 42,
            ..Default::default()
        }));
        assert_eq!(pane.lines.len(), before);
        assert_eq!(pane.last_token_total, 42);
    }
}
