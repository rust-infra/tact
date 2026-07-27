use chrono::Local;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use tact_llm::content::{ContentBlock, Message, MessageContent, Role};

use crate::{
    render::render_md::render_markdown_tui,
    widgets::state::{
        log_messages::{SystemMsgStyle, classify_system_message},
        *,
    },
};

impl App {
    pub(crate) fn add_startup_logo(&mut self) {
        let logo = [
            "  ████████╗ ",
            "  ╚══██╔══╝ ",
            "     ██║    ",
            "     ██║    ",
            "     ██║    ",
            "     ╚═╝    ",
        ];

        // Gradient: use accent color and increase brightness for each line
        let accent = self.theme.accent;
        let line_colors = match accent {
            Color::Rgb(r, g, b) => {
                let step = 15u8;
                [
                    Color::Rgb(r.saturating_sub(step * 2), g.saturating_sub(step * 2), b),
                    Color::Rgb(r.saturating_sub(step), g.saturating_sub(step), b),
                    Color::Rgb(r, g, b),
                    Color::Rgb(r.saturating_add(step / 2), g.saturating_add(step / 2), b),
                    Color::Rgb(
                        r.saturating_add(step),
                        g.saturating_add(step),
                        b.saturating_add(step / 2),
                    ),
                    Color::Rgb(
                        r.saturating_add(step * 2),
                        g.saturating_add(step * 2),
                        b.saturating_add(step),
                    ),
                ]
            }
            _ => [
                Color::Green,
                Color::LightGreen,
                Color::Green,
                Color::LightGreen,
                Color::Green,
                Color::LightGreen,
            ],
        };

        self.add_new_line();
        for (i, line) in logo.iter().enumerate() {
            let color = line_colors[i.min(line_colors.len() - 1)];
            self.append_msg(
                Line::from(Span::styled(
                    (*line).to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )),
                (*line).to_string(),
                RawMessageType::LLM,
            );
        }

        let version = env!("TACT_VERSION");
        let title = format!("  Tact Agent  v{}", version);
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            title,
            RawMessageType::LLM,
        );

        // Random startup quote
        let quotes = self.msgs().startup_quotes;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let idx = (seed as usize) % quotes.len();
        let tagline = quotes[idx];
        self.append_msg(
            Line::from(Span::styled(
                tagline.to_string(),
                Style::default()
                    .fg(self.theme.muted_fg())
                    .add_modifier(Modifier::ITALIC),
            )),
            tagline.to_string(),
            RawMessageType::LLM,
        );
        self.add_new_line();
    }

    /// Load persisted session messages into the Log area.
    /// Converts stored `Message` objects into display lines.
    /// Only `Text` blocks are rendered; `Thinking`, `ToolUse`, `ToolResult`,
    /// and `Image` blocks are skipped.
    pub(crate) fn load_history(&mut self, messages: Vec<Message>) {
        for msg in messages {
            let blocks: Vec<&ContentBlock> = match &msg.content {
                MessageContent::Blocks { content } => content.iter().collect(),
                MessageContent::Text { content } => {
                    if content.trim().is_empty() {
                        continue;
                    }
                    match msg.role {
                        Role::User => self.add_user_message(content.clone()),
                        Role::Assistant => {
                            let (lines, raw_lines) = render_markdown_tui(content, &self.theme);
                            self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
                        }
                    }
                    continue;
                }
            };

            match msg.role {
                Role::User => {
                    let texts: Vec<&str> = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if texts.is_empty() {
                        continue;
                    }
                    self.add_user_message(texts.join("\n"));
                }
                Role::Assistant => {
                    let has_text = blocks
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Text { .. }));
                    if !has_text {
                        continue;
                    }
                    self.add_new_line();
                    for block in &blocks {
                        if let ContentBlock::Text { text } = block {
                            let (lines, raw_lines) = render_markdown_tui(text, &self.theme);
                            self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
                        }
                    }
                }
            }
        }
    }

    /// Save current input state to undo stack and clear redo stack. Max 100 snapshots retained.
    pub(crate) fn save_undo(&mut self) {
        self.redo_stack.clear();
        self.undo_stack
            .push((self.input.clone(), self.input_cursor));
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
    }

    /// Add a system message, auto-color by prefix, and update scroll position.
    /// Non-system-marker messages are parsed as Markdown.
    pub(crate) fn add_system_message(&mut self, content: String) {
        if let Some(style) = SystemMsgStyle::from_line(&content) {
            let color = style.color(&self.theme);
            for line in content.split('\n') {
                let ty = classify_system_message(line);
                self.append_msg(
                    Line::from(Span::styled(line.to_string(), Style::default().fg(color))),
                    line.to_string(),
                    ty,
                );
            }
        } else {
            let ty = classify_system_message(&content);
            let (lines, raw_lines) = render_markdown_tui(&content, &self.theme);
            self.extend_msgs(lines, raw_lines, ty);
        }

        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            // u16::MAX is correctly clipped by render_log_panel based on visual line count
            self.log_scroll.offset = u16::MAX;
        }
    }

    /// Add a user input message and record it in task history.
    pub(crate) fn add_user_message(&mut self, content: String) {
        // Insert a blank line as separator first
        self.add_new_line();
        let msgs = self.msgs();
        // Style offline first so we don't hold `&self.skills_data` across `append_msg`.
        let theme = self.theme;
        let skill_names = crate::render::slash_style::skill_name_set(&self.skills_data);
        let pending: Vec<(Line<'static>, String)> = content
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                let text = if i == 0 {
                    msgs.user_msg_prefix.replace("{}", line)
                } else {
                    msgs.user_msg_cont.replace("{}", line)
                };
                let styled = crate::render::slash_style::style_user_skill_line(
                    &text,
                    &skill_names,
                    &theme,
                    msgs.user_msg_prefix,
                    msgs.user_msg_cont,
                )
                .unwrap_or_else(|| {
                    Line::from(Span::styled(
                        text.clone(),
                        Style::default().fg(theme.success),
                    ))
                });
                (styled, text)
            })
            .collect();
        for (styled, text) in pending {
            self.append_msg(styled, text, RawMessageType::LLM);
        }
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        self.task_history.push(HistoryEntry {
            task: content,
            timestamp,
            summary: String::new(),
        });
        if self.task_history.len() > 20 {
            self.task_history.remove(0);
        }
        self.refresh_tool_log_scroll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;

    #[test]
    fn add_system_message_applies_semantic_colors() {
        let mut app = make_app();

        app.add_system_message("❌ Error: boom".into());
        assert_eq!(
            app.messages.last().unwrap().spans[0].style.fg,
            Some(app.theme.error)
        );

        app.add_system_message("✓ Selected: x".into());
        assert_eq!(
            app.messages.last().unwrap().spans[0].style.fg,
            Some(app.theme.success)
        );

        app.add_system_message("  ✓ still success".into());
        assert_eq!(
            app.messages.last().unwrap().spans[0].style.fg,
            Some(app.theme.success)
        );

        app.add_system_message("📋 Copied: x".into());
        assert_eq!(
            app.messages.last().unwrap().spans[0].style.fg,
            Some(app.theme.accent)
        );
    }

    #[test]
    fn indent_prefix_uses_plain_path_not_markdown() {
        let mut app = make_app();
        app.add_system_message("  **not bold**".into());
        assert_eq!(
            app.raw_messages.last().map(String::as_str),
            Some("  **not bold**")
        );
        let spans = &app.messages.last().unwrap().spans;
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style.fg, Some(app.theme.accent));
        assert!(!spans[0].style.add_modifier.contains(Modifier::BOLD));
    }
}
