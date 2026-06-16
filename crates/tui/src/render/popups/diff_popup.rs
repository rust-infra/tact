use crate::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::popup_common::{compute_popup_layout, render_popup_scrollbar};

pub(crate) fn render_diff_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let file_path = app.diff_popup.as_ref().map(|p| p.file_path.clone());
    let file_path = match file_path {
        Some(p) => p,
        None => return,
    };

    let popup = app.diff_popup.as_mut().unwrap();

    if popup.cached_content.is_none() {
        popup.cached_content = std::fs::read_to_string(&file_path).ok();
    }
    let content = match &popup.cached_content {
        Some(c) => c,
        None => {
            let err = format!("Unable to read file: {}", file_path);
            let para = Paragraph::new(err).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(app.msgs().diff_popup_title.replace("{}", &file_path)),
            );
            frame.render_widget(para, area);
            return;
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return;
    }

    let layout = compute_popup_layout(area, total, popup.scroll, 3);

    frame.render_widget(Clear, layout.area);

    let num_width = (total + 1).to_string().len().max(3);
    let code_width = (layout.area.width as usize).saturating_sub(4 + num_width);
    let num_style = Style::default().fg(app.theme.border);
    let text_style = Style::default().fg(app.theme.fg);

    let mut text = Text::default();
    for i in layout.start_line..layout.end_line {
        let num = format!("{:>nw$}", i + 1, nw = num_width);
        let trimmed: String = lines[i].chars().take(code_width).collect();
        text.push_line(Line::from(vec![
            Span::styled(format!(" {} ", num), num_style),
            Span::styled(trimmed, text_style),
        ]));
    }

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.msgs().diff_popup_title.replace("{}", &file_path))
                .title_bottom(Line::from(vec![
                    Span::styled(
                        app.msgs().popup_copy_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                    Span::styled(
                        app.msgs().popup_close_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                    Span::styled(
                        app.msgs().popup_scroll_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                ]))
                .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, layout.area);

    render_popup_scrollbar(frame, &layout, total);

    app.mouse.diff_popup_area = layout.area;
}
