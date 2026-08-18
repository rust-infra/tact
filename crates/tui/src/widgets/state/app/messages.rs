use chrono::Local;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use tact_llm::content::{ContentBlock, Message, MessageContent, Role};

use crate::{
    i18n::{Language, Messages},
    render::cells::separator::is_task_end_separator,
    widgets::state::*,
};

/// Returns true for a per-turn stats row in any supported language, including
/// the legacy `📊 任务统计：` rows persisted before the icon was removed (old
/// sessions still need the `[copy]` affordance to keep working).
pub(crate) fn is_task_stats_line(raw: &str) -> bool {
    // The copy affordance renders before the stats body; strip it (any
    // language) before matching the prefix. Rows without a leading button
    // (the legacy format) are matched as-is.
    let body = Language::all()
        .iter()
        .filter_map(|lang| {
            raw.strip_prefix(Messages::by_language(*lang).task_stats_copy_btn)
                .map(str::trim_start)
        })
        .next()
        .unwrap_or(raw);
    Language::all()
        .iter()
        .any(|lang| body.starts_with(Messages::by_language(*lang).task_stats_prefix))
        || body.starts_with("📊 任务统计：")
}

/// Byte range `(start, end)` of the clickable copy affordance in a task-stats
/// raw row, or `None` when no localized button label is present.
pub(crate) fn find_task_stats_copy_button(raw: &str) -> Option<(usize, usize)> {
    Language::all().iter().find_map(|lang| {
        let btn = Messages::by_language(*lang).task_stats_copy_btn;
        let start = raw.find(btn)?;
        Some((start, start + btn.len()))
    })
}

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
                LogItemKind::SystemPlain(SystemMsgStyle::Default),
            );
        }

        let title = "  Tact Agent".to_string();
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            title,
            LogItemKind::SystemPlain(SystemMsgStyle::Default),
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
            LogItemKind::SystemPlain(SystemMsgStyle::Default),
        );
        self.add_new_line();
    }

    /// Load persisted session messages into the Log area.
    /// Converts stored `Message` objects into display cells or lines.
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
                        Role::Assistant => self.append_markdown(content.clone()),
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
                            self.append_markdown(text.clone());
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

    /// Append a plain system message with explicit provenance.
    ///
    /// System-ness comes from this API's caller, never from indentation or
    /// arbitrary content. Explicit marker prefixes only choose the visual
    /// system color for each line.
    pub(crate) fn add_system_message(&mut self, content: String) {
        let theme = self.theme;
        for line in content.split('\n') {
            let style = SystemMsgStyle::from_marker(line).unwrap_or(SystemMsgStyle::Default);
            self.append_msg(
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(style.color(&theme)),
                )),
                line.to_string(),
                LogItemKind::SystemPlain(style),
            );
        }
        self.scroll_after_message();
    }

    fn scroll_after_message(&mut self) {
        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            // usize::MAX is correctly clipped by render_log_panel based on visual line count
            self.scroll_log_to_bottom();
        }
    }

    /// Append a task-completion stats block right after the task-end separator.
    ///
    /// Reads the already-frozen `last_prompt_elapsed_secs` and the status-bar
    /// token/model snapshots; deliberately adds no new state (YAGNI — the data
    /// is already collected by `add_task_end_separator` / `TokenUsage` /
    /// `ModelInfo` updates). A trailing `[copy]` button copies this turn's
    /// log text (from the previous stats row, or session start, up to but not
    /// including this stats row).
    pub(crate) fn add_task_stats_block(&mut self) {
        let secs = self.last_prompt_elapsed_secs.unwrap_or(0).max(0);
        let mm_ss = format!("{:02}:{:02}", secs / 60, secs % 60);

        let mut parts = vec![format!("⏱ {mm_ss}")];
        if !self.status_bar.model_name.is_empty() {
            parts.push(self.status_bar.model_name.clone());
        }
        let tokens = self.status_bar.token_total;
        if tokens > 0 {
            let mut detail = format!("{tokens} tokens");
            let sub: Vec<String> = [
                ("prompt", self.status_bar.token_prompt),
                ("completion", self.status_bar.token_completion),
                ("cache", self.status_bar.token_cache_hit),
                ("reasoning", self.status_bar.token_reasoning),
            ]
            .into_iter()
            .filter(|(_, v)| *v > 0)
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
            if !sub.is_empty() {
                detail.push_str(&format!(" ({})", sub.join(" · ")));
            }
            parts.push(detail);
        }
        let msgs = self.msgs();
        let body = format!("{}{}", msgs.task_stats_prefix, parts.join(" · "));
        let copy_btn = msgs.task_stats_copy_btn;
        let raw = format!("{copy_btn}  {body}");
        let line = Line::from(vec![
            Span::styled(
                copy_btn.to_string(),
                Style::default()
                    .fg(self.theme.heading)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::raw("  "),
            Span::styled(body, Style::default().fg(self.theme.accent)),
        ]);
        self.append_msg(line, raw, LogItemKind::SystemPlain(SystemMsgStyle::Default));
        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.scroll_log_to_bottom();
        }
    }

    /// Copy the turn that ends at the given task-stats physical row.
    ///
    /// Range: after the previous stats line (or session start) .. `stats_phys`
    /// (exclusive). Skips blank rows, task-end separators, and other stats rows.
    pub(crate) fn copy_turn_ending_at_stats(&mut self, stats_phys: usize) {
        let Some(item) = self.log.items.get(stats_phys) else {
            return;
        };
        if !is_task_stats_line(&item.raw) {
            return;
        }
        let start = (0..stats_phys)
            .rev()
            .find(|&i| is_task_stats_line(&self.log.items[i].raw))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut parts: Vec<&str> = Vec::new();
        for i in start..stats_phys {
            let line = self.log.items[i].raw.as_str();
            if line.is_empty() || is_task_end_separator(line) || is_task_stats_line(line) {
                continue;
            }
            parts.push(line);
        }
        let text = parts.join("\n");
        if text.is_empty() {
            return;
        }
        self.copy_text_without_preview(&text);
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
            self.append_msg(styled, text, LogItemKind::User);
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
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.error)
        );

        app.add_system_message("✓ Selected: x".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.success)
        );

        app.add_system_message("  ✓ still success".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.success)
        );

        app.add_system_message("📋 Copied: x".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.accent)
        );
    }

    #[test]
    fn system_markdown_keeps_explicit_kind_and_source() {
        let mut app = make_app();
        app.append_system_markdown("  **not bold**");

        assert!(
            app.log
                .items
                .last()
                .is_some_and(|item| item.markdown_cell.is_some())
        );
        assert_eq!(
            app.log.items.last().map(|item| item.kind),
            Some(LogItemKind::SystemMarkdown)
        );
        assert_eq!(
            app.log.items.last().map(|item| item.raw.as_str()),
            Some("  **not bold**")
        );
    }

    #[test]
    fn system_tool_rows_use_explicit_provenance() {
        let mut app = make_app();
        app.append_msg(
            Line::from(Span::styled(
                "  1. inspect files",
                Style::default().fg(app.theme.accent),
            )),
            "  1. inspect files".into(),
            LogItemKind::SystemTool,
        );

        let line = app.log.items.last().expect("rendered system tool row");
        assert_eq!(line.line.spans.len(), 1);
        assert_eq!(line.line.spans[0].style.fg, Some(app.theme.accent));
        assert_eq!(
            app.log.items.last().map(|item| item.kind),
            Some(LogItemKind::SystemTool)
        );
        assert_eq!(
            app.log.items.last().map(|item| item.raw.as_str()),
            Some("  1. inspect files")
        );
    }
}
