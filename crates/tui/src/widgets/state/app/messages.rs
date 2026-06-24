use crate::render::render_md::render_markdown_tui;
use crate::widgets::state::*;
use chrono::Local;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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
            self.messages.push(Line::from(Span::styled(
                (*line).to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            self.raw_messages.push((*line).to_string());
        }

        let version = env!("TACT_VERSION");
        let title = format!("  Tact Agent  v{}", version);
        self.messages.push(Line::from(Span::styled(
            title.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        self.raw_messages.push(title);

        // Random startup quote
        let quotes = self.msgs().startup_quotes;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let idx = (seed as usize) % quotes.len();
        let tagline = quotes[idx];
        self.messages.push(Line::from(Span::styled(
            tagline.to_string(),
            Style::default()
                .fg(Color::Rgb(128, 128, 128))
                .add_modifier(Modifier::ITALIC),
        )));
        self.raw_messages.push(tagline.to_string());
        self.add_new_line();
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
        let trimmed = content.trim_start();
        let is_system = trimmed.starts_with('✓')
            || trimmed.starts_with('✗')
            || trimmed.starts_with('⚠')
            || trimmed.starts_with('📝')
            || trimmed.starts_with('❌')
            || trimmed.starts_with('✅')
            || trimmed.starts_with('▶')
            || trimmed.starts_with('🤖')
            || trimmed.starts_with("  ");

        if is_system {
            let color = if content.starts_with('✓') {
                self.theme.success
            } else if content.starts_with('✗') {
                self.theme.error
            } else if content.starts_with('⚠') {
                self.theme.warning
            } else {
                self.theme.accent
            };
            for line in content.split('\n') {
                self.messages.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(color),
                )));
                self.raw_messages.push(line.to_string());
            }
        } else {
            let (lines, raw_lines) = render_markdown_tui(&content);
            self.messages.extend(lines);
            self.raw_messages.extend(raw_lines);
        }

        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            // u16::MAX is correctly clipped by render_log_panel based on visual line count
            self.log_scroll.offset = u16::MAX;
        }
        if !self.search.term.is_empty() {
            self.update_search_matches();
        }
    }

    /// Add a user input message and record it in task history.
    pub(crate) fn add_user_message(&mut self, content: String) {
        // Insert a blank line as separator first
        self.add_new_line();
        let mut is_first = true;
        let msgs = self.msgs();
        for line in content.split('\n') {
            let text = if is_first {
                msgs.user_msg_prefix.replace("{}", line)
            } else {
                msgs.user_msg_cont.replace("{}", line)
            };
            self.messages.push(Line::from(Span::styled(
                text.clone(),
                Style::default().fg(self.theme.success),
            )));
            self.raw_messages.push(text);
            is_first = false;
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
    }
}
