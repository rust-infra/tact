use crate::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::popup_common::{compute_popup_layout, render_popup_scrollbar};

// total = 10 lines, content_height = 4, scroll = 3
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

    let layout = compute_popup_layout(area, total, popup.scroll, 3);

    frame.render_widget(Clear, layout.area);

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

    // Render code lines, truncating to popup width minus borders/padding.
    // Lines that exceed the available width get a "→" indicator at the end.
    for &line in &lines[layout.start_line..layout.end_line] {
        let chars: Vec<char> = line.chars().collect();
        let display: String = if chars.len() > layout.max_chars {
            let truncated: String = chars
                .iter()
                .take(layout.max_chars.saturating_sub(1))
                .collect();
            format!("{truncated}→") // #TODO Warn: line truncated
        } else {
            line.to_string()
        };
        text.push_line(Line::from(Span::styled(
            display,
            Style::default().fg(app.theme.fg),
        )));
    }

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", lang))
                .title_bottom(Line::from(vec![
                    Span::styled(" y:copy ", Style::default().fg(app.theme.accent)),
                    Span::styled(" j/k:scroll ", Style::default().fg(app.theme.accent)),
                    Span::styled(" Esc:close ", Style::default().fg(app.theme.accent)),
                ]))
                .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, layout.area);

    render_popup_scrollbar(frame, &layout, total);

    app.mouse.code_popup_area = layout.area;
}
