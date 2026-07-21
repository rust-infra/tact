use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::{render::util::wrap_line, widgets::state::SelectPopup};

/// Selection popup widget: displays prompt and option list centered, supports keyboard/mouse selection.
pub struct SelectPopupWidget<'a> {
    state: &'a SelectPopup,
    /// Highlight background color for selected item.
    highlight_color: Color,
    /// Normal option foreground color.
    fg_color: Color,
    /// Popup background color.
    bg_color: Color,
    /// Hint text when there are no options.
    empty_text: &'static str,
    /// Selected item prefix arrow.
    arrow: &'static str,
}

impl<'a> SelectPopupWidget<'a> {
    pub fn new(
        state: &'a SelectPopup,
        highlight_color: Color,
        fg_color: Color,
        bg_color: Color,
        empty_text: &'static str,
        arrow: &'static str,
    ) -> Self {
        SelectPopupWidget {
            state,
            highlight_color,
            fg_color,
            bg_color,
            empty_text,
            arrow,
        }
    }
}

impl Widget for SelectPopupWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let option_count = self.state.options.len().max(1) as u16;
        let max_w = area.width.saturating_sub(4).max(1);

        // ~50% of screen width; still at least fit options / a readable minimum.
        const MIN_WIDTH: u16 = 36;
        let prefix_w = if self.state.multi { 8usize } else { 4usize };
        let content_w = self
            .state
            .options
            .iter()
            .map(|o| UnicodeWidthStr::width(o.as_str()).saturating_add(prefix_w))
            .max()
            .unwrap_or(20)
            .saturating_add(4) as u16;
        let half = ((area.width as f32) * 0.5) as u16;
        let popup_width = half.max(content_w).max(MIN_WIDTH.min(max_w)).min(max_w);

        let inner_w = popup_width.saturating_sub(2).max(1) as usize;
        let prompt_style = Style::default().fg(self.fg_color);
        let mut prompt_lines = wrap_line(
            &Line::from(Span::styled(self.state.prompt.clone(), prompt_style)),
            inner_w,
        );

        let max_popup_h = area.height.saturating_sub(2).max(1);
        // borders(2) + separator(1) + options + optional multi hint(1)
        let hint_rows: u16 = if self.state.multi { 1 } else { 0 };
        let fixed = 2u16 + 1 + option_count + hint_rows;
        let max_prompt_rows = max_popup_h.saturating_sub(fixed).max(1) as usize;
        if prompt_lines.len() > max_prompt_rows {
            prompt_lines.truncate(max_prompt_rows);
            if let Some(last) = prompt_lines.last_mut() {
                *last = Line::from(Span::styled(
                    format!(
                        "{}…",
                        last.spans
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect::<String>()
                            .chars()
                            .take(inner_w.saturating_sub(1))
                            .collect::<String>()
                    ),
                    prompt_style,
                ));
            }
        }

        let prompt_rows = prompt_lines.len() as u16;
        let popup_height = (fixed + prompt_rows).min(max_popup_h);
        let popup_area =
            crate::render::popups::centered_list_popup_area(area, popup_width, popup_height);

        Clear.render(popup_area, buf);

        let title = if self.state.multi {
            " Multi-select "
        } else {
            " Select "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title)
            .style(Style::default().bg(self.bg_color));
        block.render(popup_area, buf);

        let inner = crate::render::popups::popup_inner(popup_area);
        let mut constraints = vec![
            Constraint::Length(prompt_rows),
            Constraint::Length(1),
            Constraint::Length(option_count),
        ];
        if self.state.multi {
            constraints.push(Constraint::Length(1));
        }
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        Paragraph::new(prompt_lines).render(chunks[0], buf);

        let items: Vec<ListItem> = if self.state.options.is_empty() {
            vec![ListItem::new(Span::styled(
                self.empty_text,
                Style::default().fg(Color::Gray),
            ))]
        } else {
            let selected = self
                .state
                .selected
                .min(self.state.options.len().saturating_sub(1));
            self.state
                .options
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let is_focused = i == selected;
                    let style = if is_focused {
                        Style::default().bg(self.highlight_color).fg(Color::White)
                    } else {
                        Style::default().fg(self.fg_color)
                    };
                    let cursor = if is_focused { self.arrow } else { "  " };
                    let text = if self.state.multi {
                        let mark = if self.state.checked.get(i).copied().unwrap_or(false) {
                            "[x]"
                        } else {
                            "[ ]"
                        };
                        format!("{cursor}{mark} {opt}")
                    } else {
                        format!("{cursor}{opt}")
                    };
                    ListItem::new(Span::styled(text, style))
                })
                .collect()
        };

        List::new(items)
            .block(Block::default())
            .render(chunks[2], buf);

        if self.state.multi {
            let hint = Paragraph::new(Line::from(Span::styled(
                "Space toggle · Enter confirm",
                Style::default().fg(Color::DarkGray),
            )));
            hint.render(chunks[3], buf);
        }
    }
}
