use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use crate::widgets::state::App;

//    total = 10 lines, content_height = 4, scroll = 3
//
//    lines[0]  ─┐
//    lines[1]   │ skipped (above visible area)
//    lines[2]  ─┘
//    lines[3]  ─┐ ← start_line = 3
//    lines[4]   │
//    lines[5]   │ visible in viewport
//    lines[6]  ─┘ ← end_line = min(3+4, 10) = 7
//    lines[7]  ─┐
//    lines[8]   │ skipped (below visible area)
//    lines[9]  ─┘
pub(crate) fn render_code_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match &app.code_popup {
        Some(p) => p,
        None => return,
    };
    let block = &app.code_blocks[popup.block_idx];
    let lines: Vec<&str> = block.content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return;
    }

    let popup_area = super::centered_popup_area(area);

    frame.render_widget(Clear, popup_area);

    let content_height = popup_area.height.saturating_sub(3) as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);
    let start_line = scroll;
    let end_line = (scroll + content_height).min(total);

    let mut text = Text::default();
    let title_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let lang = if popup.lang.is_empty() {
        "code"
    } else {
        &popup.lang
    };
    text.push_line(Line::from(Span::styled(
        format!("```{} ({} lines)", lang, total),
        title_style,
    )));
    text.push_line(Line::from(""));

    // Render code lines, truncating to popup width minus borders/padding
    let max_chars = popup_area.width.saturating_sub(4) as usize;
    for &line in &lines[start_line..end_line] {
        let display: String = line.chars().take(max_chars).collect();
        text.push_line(Line::from(Span::styled(
            display,
            Style::default().fg(app.theme.fg),
        )));
    }

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(app.theme.block_border_type())
                .title(format!(" {} ", lang))
                .title_bottom(Line::from(vec![
                    Span::styled(" y:copy ", Style::default().fg(app.theme.accent)),
                    Span::styled(" j/k:scroll ", Style::default().fg(app.theme.accent)),
                    Span::styled(" Esc:close ", Style::default().fg(app.theme.accent)),
                ]))
                .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, popup_area);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    app.mouse.code_popup_area = popup_area;
}
