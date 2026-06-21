use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Clear, List, ListItem},
};
use crate::widgets::state::App;

/// Render a centered file-picker popup listing files under the project root.
pub(crate) fn render_file_picker(frame: &mut Frame, area: Rect, app: &App) {
    let count = app.file_picker.options.len().max(1) as u16;
    let popup_width = 50u16.min(area.width.saturating_sub(4));
    let popup_height = (count + 4).min(area.height.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(app.msgs().file_picker_title)
        .style(Style::default().bg(app.theme.bottom_bar_bg));
    frame.render_widget(block.clone(), popup_area);

    let inner = Rect::new(
        popup_area.x + 1,
        popup_area.y + 1,
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(2),
    );

    let items: Vec<ListItem> = if app.file_picker.options.is_empty() {
        vec![ListItem::new(Span::styled(
            app.msgs().select_empty,
            Style::default().fg(Color::Gray),
        ))]
    } else {
        let selected = app
            .file_picker
            .selected
            .min(app.file_picker.options.len().saturating_sub(1));
        app.file_picker
            .options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let is_selected = i == selected;
                let style = if is_selected {
                    Style::default().bg(app.theme.highlight).fg(Color::White)
                } else {
                    Style::default().fg(app.theme.fg)
                };
                let prefix = if is_selected { app.msgs().select_arrow } else { "  " };
                ListItem::new(Span::styled(format!("{}{}", prefix, opt), style))
            })
            .collect()
    };

    let list = List::new(items).block(Block::default());
    frame.render_widget(list, inner);
}
