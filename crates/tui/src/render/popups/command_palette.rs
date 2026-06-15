use ratatui::{Frame, layout::Rect, style::{Color, Style}, text::Span, widgets::{Block, Borders, Clear, List, ListItem}};
use crate::state::{App, PALETTE_COMMANDS};

pub(crate) fn render_command_palette(frame: &mut Frame, area: Rect, app: &App) {
    let filter = app.cmd_line.to_lowercase();
    let filtered: Vec<(usize, &(&str, &str))> = PALETTE_COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, (cmd, desc))| {
            filter.is_empty()
                || cmd.to_lowercase().contains(&filter)
                || desc.to_lowercase().contains(&filter)
        })
        .collect();

    let count = filtered.len().max(1) as u16;
    let popup_width = 44u16;
    let popup_height = count + 4;
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(app.msgs().palette_title.replace("{}", &app.cmd_line))
        .style(Style::default().bg(app.theme.bottom_bar_bg));
    frame.render_widget(block.clone(), popup_area);

    let inner = Rect::new(
        popup_area.x + 1,
        popup_area.y + 1,
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(2),
    );

    let items: Vec<ListItem> = if filtered.is_empty() {
        vec![ListItem::new(Span::styled(
            app.msgs().palette_empty,
            Style::default().fg(Color::Gray),
        ))]
    } else {
        filtered
            .iter()
            .enumerate()
            .map(|(i, (_orig_idx, (cmd, _desc)))| {
                let is_selected = i == app.palette_selected.min(filtered.len().saturating_sub(1));
                let style = if is_selected {
                    Style::default().bg(app.theme.highlight).fg(Color::White)
                } else {
                    Style::default().fg(app.theme.fg)
                };
                let text = format!("  {:<12} {}", cmd, app.localize_cmd_desc(cmd));
                ListItem::new(Span::styled(text, style))
            })
            .collect()
    };

    let list = List::new(items).block(Block::default());
    frame.render_widget(list, inner);
}
