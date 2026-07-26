//! Subagent popup: full conversation text during run and after completion.
//!
//! During live execution the content is read from the active tool block's
//! `live_output`. After completion [`detail_full`] contains the preserved
//! full conversation (populated in `on_step_finished`).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

use crate::widgets::state::App;

pub(crate) fn render_subagent_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match app.subagent_popup.as_ref() {
        Some(p) => p.clone(),
        None => return,
    };

    let code_bg = app.theme.code_block_bg();
    let code_fg = app.theme.code_block_fg();

    // Determine live/finished dynamically from current state.
    let is_live = app.tools.active.iter().any(|a| a.tool_id == popup.tool_id);

    let raw_text = if is_live {
        app.tools
            .active
            .iter()
            .find(|a| a.tool_id == popup.tool_id)
            .map(|a| a.live_output.detail_text())
            .unwrap_or_default()
    } else {
        app.tools
            .blocks
            .iter()
            .find(|b| b.tool_id == popup.tool_id)
            .and_then(|b| b.output.detail_full.clone())
            .unwrap_or_default()
    };

    let total_lines = raw_text.lines().count();

    // Plain text with line numbers for both live and finished states.
    let num_width = (total_lines + 1).to_string().len().max(3);
    let num_style = Style::default().fg(app.theme.muted_fg()).bg(code_bg);
    let text_style = Style::default().fg(code_fg).bg(code_bg);

    let mut body_lines = Vec::new();
    for (i, line) in raw_text.lines().enumerate() {
        let num = format!("{:>nw$} ", i + 1, nw = num_width);
        body_lines.push(Line::from(vec![
            Span::styled(num, num_style),
            Span::styled(line.to_string(), text_style),
        ]));
    }
    if raw_text.ends_with('\n') {
        body_lines.push(Line::from(Span::styled("", text_style)));
    }
    let body = Text::from(body_lines);

    let popup_area = super::centered_popup_area(area);
    frame.render_widget(Clear, popup_area);

    let body_area = Rect::new(
        popup_area.x.saturating_add(1),
        popup_area.y.saturating_add(1),
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(3),
    );

    let content_height = body_area.height as usize;
    let max_scroll = total_lines.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);

    let title = if is_live {
        format!(" {} (live, {} lines) ", popup.title, total_lines)
    } else {
        format!(" {} ({} lines) ", popup.title, total_lines)
    };

    let mut visible_body = Text::default();
    for line in body.lines.iter().skip(scroll).take(content_height) {
        visible_body.push_line(line.clone());
    }

    let para = Paragraph::new(visible_body).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(app.theme.block_border_type())
            .border_style(Style::default().fg(app.theme.code_card_border()))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(app.theme.code_card_title_fg())
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(code_bg)),
    );

    frame.render_widget(para, popup_area);

    let scrollbar = Scrollbar::default()
        .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total_lines)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);
}
