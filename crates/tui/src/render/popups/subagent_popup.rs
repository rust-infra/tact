//! Subagent popup: live text during run, markdown after completion.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

use crate::{
    render::render_md::render_markdown_tui,
    widgets::state::App,
};

pub(crate) fn render_subagent_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match app.subagent_popup.as_ref() {
        Some(p) => p.clone(),
        None => return,
    };

    let code_bg = app.theme.code_block_bg();
    let code_fg = app.theme.code_block_fg();

    // Resolve content: live from active tool block, or finished from completed block.
    let (raw_text, is_live) = if popup.live {
        // Running — read live output from active block.
        let text = app
            .tools
            .active
            .iter()
            .find(|a| {
                !popup.title.is_empty() && a.output.tool_name == "spawn_subagent"
            })
            .map(|a| a.live_output.detail_text())
            .unwrap_or_default();
        (text, true)
    } else {
        // Finished — read detail_full from completed block.
        let text = app
            .tools
            .blocks
            .iter()
            .find(|b| b.output.tool_name == "spawn_subagent")
            .and_then(|b| b.output.detail_full.clone())
            .unwrap_or_default();
        (text, false)
    };

    let total_lines = raw_text.lines().count();

    // Render markdown for finished popups (cached via a simple mutable reassign).
    let body: Text = if is_live {
        // Plain text with line numbers for live mode.
        let num_width = (total_lines + 1).to_string().len().max(3);
        let num_style = Style::default().fg(app.theme.muted_fg()).bg(code_bg);
        let text_style = Style::default().fg(code_fg).bg(code_bg);

        let mut lines = Vec::new();
        for (i, line) in raw_text.lines().enumerate() {
            let num = format!("{:>nw$} ", i + 1, nw = num_width);
            lines.push(Line::from(vec![
                Span::styled(num, num_style),
                Span::styled(line.to_string(), text_style),
            ]));
        }
        Text::from(lines)
    } else {
        // Markdown rendering for finished output.
        let (styled, _) = render_markdown_tui(&raw_text, &app.theme);
        // Re-style lines to use popup background.
        let styled: Vec<Line> = styled
            .into_iter()
            .map(|line| {
                let spans: Vec<Span> = line
                    .spans
                    .into_iter()
                    .map(|s| {
                        let mut style = s.style;
                        if style.bg.is_none() || style.bg == Some(ratatui::style::Color::Reset) {
                            style = style.bg(code_bg);
                        }
                        Span::styled(s.content.into_owned(), style)
                    })
                    .collect();
                Line::from(spans)
            })
            .collect();
        Text::from(styled)
    };

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
