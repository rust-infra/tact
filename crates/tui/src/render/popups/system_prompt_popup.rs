use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use crate::widgets::state::App;

pub(crate) fn render_system_prompt_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(popup) = app.system_prompt_popup.as_ref() else {
        return;
    };
    let popup_area = super::centered_popup_area(area);
    frame.render_widget(Clear, popup_area);
    let lines = popup.rendered.clone();
    let total = lines.len().max(1);
    let content_height = popup_area.height.saturating_sub(3) as usize;
    let scroll = (popup.scroll as usize).min(total.saturating_sub(1));
    let text = Text::from(lines);
    let title_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let paragraph = Paragraph::new(text)
        .scroll((scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(format!(" {} ", popup.title), title_style))
                .title_bottom(Line::from(vec![
                    Span::styled(" j/k:scroll ", Style::default().fg(app.theme.accent)),
                    Span::styled(" Esc:close ", Style::default().fg(app.theme.accent)),
                ]))
                .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup_area);
    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut scrollbar_state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut scrollbar_state);
}
