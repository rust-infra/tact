use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem},
};

pub(crate) fn render_slash_command_popup(frame: &mut Frame, area: Rect, app: &App) {
    let slash = &app.slash_command;
    if !slash.active {
        return;
    }

    let commands: Vec<_> = app.palette_commands().copied().collect();
    let filtered = slash.matched_commands(&app.input, app.input_cursor, &commands);
    let n = filtered.len();
    if n == 0 {
        let hint_area = super::centered_list_popup_area(area, 30, 5);
        let hint_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Clear, hint_area);
        frame.render_widget(&hint_block, hint_area);
        let inner = hint_block.inner(hint_area);
        frame.buffer_mut().set_line(
            inner.x,
            inner.y + 1,
            &Line::from(Span::styled(
                "No matching command",
                Style::default().fg(Color::Gray),
            )),
            inner.width,
        );
        return;
    }

    let selected = slash.selected.min(n.saturating_sub(1));
    let max_visible = 8usize;
    let list_height = n.min(max_visible) as u16;
    let popup_width: u16 = 42;

    let popup_area = super::centered_list_popup_area(area, popup_width, list_height + 2);

    // Determine visible range (scroll if needed)
    let offset = if n > max_visible && selected >= max_visible {
        selected - max_visible + 1
    } else {
        0
    };

    // Highlight color from theme
    let accent = Color::Cyan;

    // Build items
    let items: Vec<ListItem> = filtered[offset..(offset + max_visible).min(n)]
        .iter()
        .enumerate()
        .map(|(i, &(_idx, (cmd, desc), _score))| {
            let global_idx = offset + i;
            let prefix = if global_idx == selected { "▶ " } else { "  " };
            let content = if popup_width > 30 {
                Line::from(vec![
                    Span::styled(
                        format!("{prefix}/{cmd}"),
                        if global_idx == selected {
                            Style::default().fg(accent).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                    Span::raw("  "),
                    Span::styled(desc, Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(Span::styled(
                    format!("{prefix}/{cmd}"),
                    if global_idx == selected {
                        Style::default().fg(accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ))
            };
            ListItem::new(content)
        })
        .collect();

    let block = Block::default()
        .title(Span::styled(
            "Commands",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent));

    let list = List::new(items).block(block);

    frame.render_widget(Clear, popup_area);
    frame.render_widget(list, popup_area);
}
